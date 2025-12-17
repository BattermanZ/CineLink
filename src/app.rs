use crate::anilist::{AniListApi, AniListClient};
use crate::notion::{self, NotionApi, NotionClient};
use crate::notion_fallback::fallback_schema;
use crate::tmdb::{self, TmdbApi, TmdbClient};
use anyhow::Result;
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Utc};
use constant_time_eq::constant_time_eq;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

const MAX_BODY_BYTES: usize = 1024 * 1024; // 1MB safety cap
const PER_IP_LIMIT: u32 = 60; // per minute
const PER_IP_BURST: u32 = 10;
const GLOBAL_LIMIT: u32 = 200; // per minute
const GLOBAL_BURST: u32 = 20;
const MAX_SKEW_SECS: i64 = 300; // 5 minutes freshness window
const DEDUPE_TTL_SECS: i64 = 600; // 10 minutes
const MAX_CONCURRENT_JOBS: usize = 8;
const MAX_RATE_LIMIT_ENTRIES: usize = 10_000;
const MAX_DEDUPE_ENTRIES: usize = 10_000;

#[derive(Clone)]
pub struct AppState {
    pub notion: Arc<dyn NotionApi>,
    pub tmdb: Arc<dyn TmdbApi>,
    pub anilist: Arc<dyn AniListApi>,
    pub title_property: String,
    pub schema: Arc<notion::PropertySchema>,
    pub signing_secret: String,
    pub rate_limits: Arc<Mutex<HashMap<String, WindowCounter>>>,
    pub global_limit: Arc<Mutex<WindowCounter>>,
    pub recent_events: Arc<Mutex<HashMap<String, i64>>>,
    pub processing_sem: Arc<Semaphore>,
}

#[derive(Clone, Debug)]
pub struct WindowCounter {
    pub window: u64,
    pub count: u32,
}

pub async fn run_server() -> Result<()> {
    let notion: Arc<dyn NotionApi> = Arc::new(NotionClient::from_env()?);
    let schema = match notion.fetch_property_schema().await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::warn!("Failed to fetch Notion schema, using fallback: {}", e);
            Arc::new(fallback_schema())
        }
    };
    let title_property = schema
        .title_property
        .clone()
        .unwrap_or_else(|| "Name".to_string());
    info!("Using title property: {}", title_property);

    let tmdb: Arc<dyn TmdbApi> = Arc::new(TmdbClient::from_env()?);
    let anilist: Arc<dyn AniListApi> = Arc::new(AniListClient::new()?);
    let signing_secret = env::var("NOTION_WEBHOOK_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("NOTION_WEBHOOK_SECRET must be set"))?;
    info!("Webhook signature will use NOTION_WEBHOOK_SECRET");

    let rate_limits = Arc::new(Mutex::new(HashMap::new()));
    let global_limit = Arc::new(Mutex::new(WindowCounter {
        window: 0,
        count: 0,
    }));
    let recent_events = Arc::new(Mutex::new(HashMap::new()));
    let processing_sem = Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS));

    let state = AppState {
        notion,
        tmdb,
        anilist,
        title_property,
        schema,
        signing_secret,
        rate_limits,
        global_limit,
        recent_events,
        processing_sem,
    };

    let app = build_router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3146));
    info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", post(handle_webhook))
        .route("/health", get(health))
        .with_state(state)
}

async fn health() -> &'static str {
    "OK"
}

async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let ip = extract_ip(&headers);
    if !check_rate_limit(&state, &ip).await || !check_global_rate_limit(&state).await {
        warn!("Rate limit exceeded for {}", ip);
        return StatusCode::TOO_MANY_REQUESTS;
    }

    if body.len() > MAX_BODY_BYTES {
        warn!(
            "Rejecting request: body too large ({} bytes > {} bytes)",
            body.len(),
            MAX_BODY_BYTES
        );
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    // Enforce content type
    let content_type_ok = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        == Some(true);
    if !content_type_ok {
        warn!(
            "Rejecting request: unsupported content-type {:?}",
            headers.get(header::CONTENT_TYPE)
        );
        return StatusCode::UNSUPPORTED_MEDIA_TYPE;
    }

    if !verify_notion_signature(&headers, &body, &state.signing_secret) {
        warn!("Webhook signature verification failed");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("Rejecting request: invalid JSON body: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    if payload.get("type").and_then(|v| v.as_str()) != Some("page.properties_updated") {
        warn!("Ignoring event with unsupported type");
        return StatusCode::OK;
    }

    if !is_fresh_timestamp(&payload) {
        warn!("Rejecting request: stale or missing timestamp");
        return StatusCode::BAD_REQUEST;
    }

    if let Some(event_id) = payload.get("id").and_then(|v| v.as_str()) {
        if !dedupe_event(&state, event_id).await {
            return StatusCode::OK;
        }
    }

    let updated_raw = payload
        .get("data")
        .and_then(|d| d.get("updated_properties"))
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();

    let updated_decoded: Vec<String> = updated_raw
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| match urlencoding::decode(s) {
            Ok(decoded) => decoded.into_owned(),
            Err(_) => s.to_string(),
        })
        .collect();

    let should_process = updated_raw.iter().any(|v| {
        v.as_str() == Some("Siv%5D")
            || updated_decoded.iter().any(|p| {
                let lower = p.to_lowercase();
                lower == "title" || lower == "season"
            })
    });
    if !should_process {
        return StatusCode::OK;
    }

    let page_id = match payload
        .get("entity")
        .and_then(|e| e.get("id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id.to_string(),
        None => return StatusCode::BAD_REQUEST,
    };

    let event_id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    debug!(
        page_id = %page_id,
        event_id = ?event_id,
        "Webhook accepted; queued page check"
    );

    let state_for_task = state.clone();
    let page_id_for_task = page_id.clone();
    tokio::spawn(async move {
        let _permit = match state_for_task.processing_sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };

        if let Err(err) = process_page(&state_for_task, &page_id_for_task, event_id.as_deref())
            .await
            .map(|_| ())
        {
            error!("Failed to process page: {:?}", err);
        }
    });

    StatusCode::OK
}

pub async fn process_page_backfill_tv(state: &AppState, page_id: &str) -> Result<bool> {
    process_page_inner(state, page_id, None, false).await
}

async fn process_page(state: &AppState, page_id: &str, event_id: Option<&str>) -> Result<bool> {
    process_page_inner(state, page_id, event_id, true).await
}

async fn process_page_inner(
    state: &AppState,
    page_id: &str,
    event_id: Option<&str>,
    require_semicolon: bool,
) -> Result<bool> {
    let page = state.notion.fetch_page(page_id).await?;
    let props = page
        .get("properties")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow::anyhow!("Page has no properties"))?;

    // Enrich schema from live page properties (handles cases where DB schema is unavailable).
    let mut schema = (*state.schema).clone();
    notion::merge_schema_from_props(&mut schema, props);

    let raw_title = notion::extract_title(props, &state.title_property).unwrap_or_default();

    enum TriggerKind {
        Tmdb,
        AniList,
    }

    let (trigger_kind, clean_title) = if require_semicolon {
        let (kind, trimmed) = if raw_title.ends_with(';') {
            (TriggerKind::Tmdb, raw_title.trim_end_matches(';'))
        } else if raw_title.ends_with('=') {
            (TriggerKind::AniList, raw_title.trim_end_matches('='))
        } else {
            return Ok(false);
        };
        info!("Received trigger for page '{}'", raw_title);
        (kind, trimmed.trim().to_string())
    } else {
        if raw_title.trim().is_empty() || raw_title.ends_with(';') {
            return Ok(false);
        }
        info!("Backfill updating page '{}'", raw_title);
        (TriggerKind::Tmdb, raw_title.trim().to_string())
    };

    let type_value = notion::extract_select(props, "Type");
    let is_tv = type_value
        .as_deref()
        .map(|t| t.to_lowercase().contains("tv"))
        .unwrap_or(false);

    let season_str = notion::extract_select(props, "Season")
        .or_else(|| notion::extract_rich_text(props, "Season"));
    let season_number_parsed = season_str.as_deref().and_then(tmdb::parse_season_number);

    if matches!(trigger_kind, TriggerKind::AniList) {
        return process_anilist_page(
            state,
            page_id,
            event_id,
            raw_title,
            &clean_title,
            season_number_parsed,
            &schema,
        )
        .await;
    }

    let imdb_hint = tmdb::parse_imdb_id(&clean_title);
    let mut resolved_id: Option<i32> = None;
    let mut forced_tv = is_tv;

    if let Some(imdb) = imdb_hint {
        let (movie_id, tv_id) = state.tmdb.lookup_imdb(&imdb).await?;
        if forced_tv {
            if let Some(id) = tv_id {
                resolved_id = Some(id);
            } else if let Some(id) = movie_id {
                resolved_id = Some(id);
                forced_tv = false;
            }
        } else if let Some(id) = movie_id {
            resolved_id = Some(id);
        } else if let Some(id) = tv_id {
            resolved_id = Some(id);
            forced_tv = true;
        }
    }

    let tmdb_media = if forced_tv {
        let season = match season_number_parsed {
            Some(s) => s,
            None => {
                warn!("TV item missing or invalid season, skipping");
                return Ok(false);
            }
        };
        let show_id = match resolved_id {
            Some(id) => id,
            None => match state.tmdb.resolve_tv_id(&clean_title).await {
                Ok(id) => id,
                Err(e) => {
                    warn!("No TMDB match for TV '{}': {}", clean_title, e);
                    set_error_title(
                        &state.notion,
                        page_id,
                        &state.title_property,
                        &schema,
                        raw_title,
                        "No TMDB TV match",
                    )
                    .await?;
                    return Ok(false);
                }
            },
        };
        info!(
            "Fetching TMDB data for TV '{}' (tmdb id {}), season {}",
            clean_title, show_id, season
        );
        match state.tmdb.fetch_tv_season(show_id, season).await {
            Ok(data) => data,
            Err(e) => {
                warn!(
                    "Failed to fetch TMDB TV season for '{}': {}",
                    clean_title, e
                );
                set_error_title(
                    &state.notion,
                    page_id,
                    &state.title_property,
                    &schema,
                    raw_title,
                    "No TMDB TV match",
                )
                .await?;
                return Ok(false);
            }
        }
    } else {
        let movie_id = match resolved_id {
            Some(id) => id,
            None => match state.tmdb.resolve_movie_id(&clean_title).await {
                Ok(id) => id,
                Err(e) => {
                    warn!("No TMDB match for Movie '{}': {}", clean_title, e);
                    set_error_title(
                        &state.notion,
                        page_id,
                        &state.title_property,
                        &schema,
                        raw_title,
                        "No TMDB movie match",
                    )
                    .await?;
                    return Ok(false);
                }
            },
        };
        info!(
            "Fetching TMDB data for Movie '{}' (tmdb id {})",
            clean_title, movie_id
        );
        match state.tmdb.fetch_movie(movie_id).await {
            Ok(data) => data,
            Err(e) => {
                warn!("Failed to fetch TMDB movie for '{}': {}", clean_title, e);
                set_error_title(
                    &state.notion,
                    page_id,
                    &state.title_property,
                    &schema,
                    raw_title,
                    "No TMDB movie match",
                )
                .await?;
                return Ok(false);
            }
        }
    };

    info!("Matched '{}' -> '{}'", raw_title, tmdb_media.name);
    debug!(
        page_id = %page_id,
        event_id = ?event_id,
        original_title = %raw_title,
        updated_title = %tmdb_media.name,
        tmdb_id = tmdb_media.id,
        "Processing context"
    );

    let mut updates = serde_json::Map::new();
    notion::set_title(
        &mut updates,
        &state.title_property,
        &tmdb_media.name,
        &schema,
    );

    if let Some(eng) = tmdb_media.eng_name.clone() {
        notion::set_value(
            &mut updates,
            "Eng Name",
            Some(notion::ValueInput::Text(eng)),
            &schema,
        );
    }
    if tmdb_media.eng_name.is_none() {
        if let Some(orig) = tmdb_media.original_title.clone() {
            if orig.trim() != tmdb_media.name.trim() {
                notion::set_value(
                    &mut updates,
                    "Original Title",
                    Some(notion::ValueInput::Text(orig)),
                    &schema,
                );
            }
        }
    }
    notion::set_value(
        &mut updates,
        "Synopsis",
        tmdb_media.synopsis.map(notion::ValueInput::Text),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Genre",
        Some(notion::ValueInput::StringList(tmdb_media.genres)),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Cast",
        Some(notion::ValueInput::StringList(tmdb_media.cast)),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Director",
        Some(notion::ValueInput::StringList(tmdb_media.director)),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Content Rating",
        tmdb_media.content_rating.map(notion::ValueInput::Text),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Country of origin",
        Some(notion::ValueInput::StringList(tmdb_media.country_of_origin)),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Language",
        tmdb_media.language.map(notion::ValueInput::Text),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Release Date",
        tmdb_media.release_date.map(notion::ValueInput::Date),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Year",
        tmdb_media.year.map(notion::ValueInput::Text),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "Runtime",
        tmdb_media
            .runtime_minutes
            .map(|r| notion::ValueInput::Number(r as f64)),
        &schema,
    );
    if let Some(episodes) = tmdb_media.episodes {
        notion::set_value(
            &mut updates,
            "Episodes",
            Some(notion::ValueInput::Number(episodes as f64)),
            &schema,
        );
    }
    notion::set_value(
        &mut updates,
        "Trailer",
        tmdb_media.trailer.map(notion::ValueInput::Url),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "IMG",
        tmdb_media.poster.clone().map(notion::ValueInput::Url),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "IMDb Page",
        tmdb_media.imdb_page.map(notion::ValueInput::Url),
        &schema,
    );
    notion::set_value(
        &mut updates,
        "ID",
        Some(notion::ValueInput::Number(tmdb_media.id as f64)),
        &schema,
    );

    // Prepare icon/cover using poster/backdrop if available.
    let icon = tmdb_media.poster.as_ref().map(|url| {
        json!({
            "type": "external",
            "external": { "url": url }
        })
    });
    let cover = tmdb_media.backdrop.as_ref().map(|url| {
        json!({
            "type": "external",
            "external": { "url": url }
        })
    });

    info!("Updating Notion page '{}'", tmdb_media.name);
    state
        .notion
        .update_page(page_id, updates, icon, cover)
        .await?;
    info!(
        "Finished update for page '{}' -> '{}'",
        raw_title, tmdb_media.name
    );
    Ok(true)
}

async fn process_anilist_page(
    state: &AppState,
    page_id: &str,
    event_id: Option<&str>,
    raw_title: String,
    query: &str,
    season: Option<i32>,
    schema: &notion::PropertySchema,
) -> Result<bool> {
    let anime_id = match state.anilist.resolve_anime_id(query, season).await {
        Ok(id) => id,
        Err(e) => {
            warn!("No AniList match for Anime '{}': {}", query, e);
            set_error_title(
                &state.notion,
                page_id,
                &state.title_property,
                schema,
                raw_title,
                "No AniList match",
            )
            .await?;
            return Ok(false);
        }
    };

    debug!(
        page_id = %page_id,
        event_id = ?event_id,
        anilist_id = anime_id,
        season = ?season,
        query = query,
        "AniList resolved id"
    );
    info!(
        "Fetching AniList data for Anime '{}' (anilist id {})",
        query, anime_id
    );
    let anime = match state.anilist.fetch_anime(anime_id).await {
        Ok(data) => data,
        Err(e) => {
            warn!("Failed to fetch AniList anime for '{}': {}", query, e);
            set_error_title(
                &state.notion,
                page_id,
                &state.title_property,
                schema,
                raw_title,
                "No AniList match",
            )
            .await?;
            return Ok(false);
        }
    };

    let mut updates = serde_json::Map::new();
    let updated_title = crate::anilist::strip_trailing_season_suffix(&anime.name);
    let original_title = anime
        .original_title
        .as_deref()
        .map(crate::anilist::strip_trailing_season_suffix);

    notion::set_title(&mut updates, &state.title_property, &updated_title, schema);

    // Explicitly blank Eng Name (anime title is already the "actual" title).
    notion::set_value(
        &mut updates,
        "Eng Name",
        Some(notion::ValueInput::Text(String::new())),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Original Title",
        original_title.map(notion::ValueInput::Text),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Synopsis",
        anime.synopsis.map(notion::ValueInput::Text),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Genre",
        Some(notion::ValueInput::StringList(with_anime_tags(
            anime.genres,
        ))),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Cast",
        Some(notion::ValueInput::StringList(anime.cast)),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Director",
        Some(notion::ValueInput::StringList(anime.director)),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Content Rating",
        Some(notion::ValueInput::Text(anime.content_rating)),
        schema,
    );
    if let Some(country) = anime.country_of_origin {
        notion::set_value(
            &mut updates,
            "Country of origin",
            Some(notion::ValueInput::StringList(vec![country])),
            schema,
        );
    }
    notion::set_value(
        &mut updates,
        "Language",
        anime.language.map(notion::ValueInput::Text),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Release Date",
        anime.release_date.map(notion::ValueInput::Date),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Year",
        anime.year.map(notion::ValueInput::Text),
        schema,
    );
    notion::set_value(
        &mut updates,
        "Runtime",
        anime
            .runtime_minutes
            .map(|r| notion::ValueInput::Number(r as f64)),
        schema,
    );
    if let Some(episodes) = anime.episodes {
        notion::set_value(
            &mut updates,
            "Episodes",
            Some(notion::ValueInput::Number(episodes as f64)),
            schema,
        );
    }
    notion::set_value(
        &mut updates,
        "Trailer",
        anime.trailer.map(notion::ValueInput::Url),
        schema,
    );
    notion::set_value(
        &mut updates,
        "IMG",
        anime.poster.clone().map(notion::ValueInput::Url),
        schema,
    );
    notion::set_value(
        &mut updates,
        "IMDb Page",
        anime.imdb_page.map(notion::ValueInput::Url),
        schema,
    );
    notion::set_value(
        &mut updates,
        "ID",
        Some(notion::ValueInput::Number(anime.id as f64)),
        schema,
    );

    let icon = anime.poster.as_ref().map(|url| {
        json!({
            "type": "external",
            "external": { "url": url }
        })
    });
    let cover = anime.backdrop.as_ref().map(|url| {
        json!({
            "type": "external",
            "external": { "url": url }
        })
    });

    debug!(
        page_id = %page_id,
        event_id = ?event_id,
        "Updating Notion page from AniList"
    );
    info!("Updating Notion page from AniList");
    state
        .notion
        .update_page(page_id, updates, icon, cover)
        .await?;
    info!(
        "Finished AniList update '{}' -> '{}'",
        raw_title, updated_title
    );
    Ok(true)
}

fn with_anime_tags(mut genres: Vec<String>) -> Vec<String> {
    // Always tag AniList-sourced pages so they're easy to filter in Notion.
    let tags = ["Anime", "Animation"];
    let tag_lc: std::collections::HashSet<String> = tags.iter().map(|t| t.to_lowercase()).collect();
    genres.retain(|g| !tag_lc.contains(&g.to_lowercase()));
    for tag in tags.into_iter().rev() {
        genres.insert(0, tag.to_string());
    }
    genres
}

async fn set_error_title(
    notion: &Arc<dyn NotionApi>,
    page_id: &str,
    title_property: &str,
    schema: &notion::PropertySchema,
    original_title: String,
    message: &str,
) -> Result<()> {
    let mut props = serde_json::Map::new();
    let new_title = format!("{} | {}", original_title, message);
    notion::set_title(&mut props, title_property, &new_title, schema);
    notion
        .update_page(page_id, props, None, None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to set error title: {}", e))
}

fn verify_notion_signature(headers: &HeaderMap, body: &[u8], secret: &str) -> bool {
    let Some(sig_header) = headers
        .get("x-notion-signature")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let sig_hex = sig_header.strip_prefix("sha256=").unwrap_or(sig_header);
    let Ok(expected) = hex::decode(sig_hex) else {
        return false;
    };

    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let computed = mac.finalize().into_bytes();

    expected.len() == computed.len() && constant_time_eq(&computed, &expected)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        term.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Shutdown signal received (Ctrl+C)");
        }
        _ = terminate => {
            info!("Shutdown signal received (SIGTERM)");
        }
    }
}

fn extract_ip(headers: &HeaderMap) -> String {
    headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-real-ip"))
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn check_rate_limit(state: &AppState, ip: &str) -> bool {
    let window = (Utc::now().timestamp() / 60) as u64;
    let mut guards = state.rate_limits.lock().await;
    if guards.len() > MAX_RATE_LIMIT_ENTRIES {
        guards.retain(|_, v| v.window == window);
    }
    let entry = guards
        .entry(ip.to_string())
        .or_insert(WindowCounter { window, count: 0 });
    if entry.window != window {
        entry.window = window;
        entry.count = 0;
    }
    if entry.count >= PER_IP_LIMIT + PER_IP_BURST {
        return false;
    }
    entry.count += 1;
    true
}

async fn check_global_rate_limit(state: &AppState) -> bool {
    let window = (Utc::now().timestamp() / 60) as u64;
    let mut guard = state.global_limit.lock().await;
    if guard.window != window {
        guard.window = window;
        guard.count = 0;
    }
    if guard.count >= GLOBAL_LIMIT + GLOBAL_BURST {
        return false;
    }
    guard.count += 1;
    true
}

fn is_fresh_timestamp(payload: &serde_json::Value) -> bool {
    let ts_str = match payload.get("timestamp").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => return false,
    };
    let parsed: DateTime<Utc> = match ts_str.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let now = Utc::now();
    let diff = (now - parsed).num_seconds().abs();
    diff <= MAX_SKEW_SECS
}

async fn dedupe_event(state: &AppState, event_id: &str) -> bool {
    let now = Utc::now().timestamp();
    let mut guard = state.recent_events.lock().await;
    guard.retain(|_, ts| now - *ts <= DEDUPE_TTL_SECS);
    if guard.len() > MAX_DEDUPE_ENTRIES {
        guard.clear();
    }
    if guard.contains_key(event_id) {
        return false;
    }
    guard.insert(event_id.to_string(), now);
    true
}
