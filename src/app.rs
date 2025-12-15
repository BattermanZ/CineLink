use crate::notion::{self, NotionApi, NotionClient};
use crate::notion_fallback::fallback_schema;
use crate::tmdb::{self, TmdbApi, TmdbClient};
use anyhow::Result;
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    routing::post,
    Router,
};
use chrono::{DateTime, Utc};
use constant_time_eq::constant_time_eq;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const MAX_BODY_BYTES: usize = 1024 * 1024; // 1MB safety cap
const PER_IP_LIMIT: u32 = 60; // per minute
const PER_IP_BURST: u32 = 10;
const GLOBAL_LIMIT: u32 = 200; // per minute
const GLOBAL_BURST: u32 = 20;
const MAX_SKEW_SECS: i64 = 300; // 5 minutes freshness window

#[derive(Clone)]
pub struct AppState {
    pub notion: Arc<dyn NotionApi>,
    pub tmdb: Arc<dyn TmdbApi>,
    pub title_property: String,
    pub schema: Arc<notion::PropertySchema>,
    pub signing_secret: String,
    pub rate_limits: Arc<Mutex<HashMap<String, WindowCounter>>>,
    pub global_limit: Arc<Mutex<WindowCounter>>,
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

    let state = AppState {
        notion,
        tmdb,
        title_property,
        schema,
        signing_secret,
        rate_limits,
        global_limit,
    };

    let app = build_router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3146));
    info!("Listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", post(handle_webhook))
        .with_state(state)
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

    // Enforce content type and version
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
        Some(id) => id,
        None => return StatusCode::BAD_REQUEST,
    };

    if let Err(err) = process_page(&state, page_id).await {
        error!("Failed to process page {}: {:?}", page_id, err);
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    StatusCode::OK
}

async fn process_page(state: &AppState, page_id: &str) -> Result<()> {
    let page = state.notion.fetch_page(page_id).await?;
    let props = page
        .get("properties")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow::anyhow!("Page has no properties"))?;

    // Enrich schema from live page properties (handles cases where DB schema is unavailable).
    let mut schema = (*state.schema).clone();
    notion::merge_schema_from_props(&mut schema, props);

    let raw_title = notion::extract_title(props, &state.title_property).unwrap_or_default();

    if !raw_title.ends_with(';') {
        return Ok(());
    }

    let clean_title = raw_title.trim_end_matches(';').trim().to_string();
    info!("Received trigger for page '{}'", raw_title);

    let type_value = notion::extract_select(props, "Type");
    let is_tv = type_value
        .as_deref()
        .map(|t| t.to_lowercase().contains("tv"))
        .unwrap_or(false);

    let season_str = notion::extract_select(props, "Season")
        .or_else(|| notion::extract_rich_text(props, "Season"));
    let season_number_parsed = season_str.as_deref().and_then(tmdb::parse_season_number);

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
                return Ok(());
            }
        };
        info!(
            "Fetching TMDB data for TV '{}', season {} (matched id {:?})",
            clean_title, season, resolved_id
        );
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
                    return Ok(());
                }
            },
        };
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
                return Ok(());
            }
        }
    } else {
        info!(
            "Fetching TMDB data for Movie '{}' (matched id {:?})",
            clean_title, resolved_id
        );
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
                    return Ok(());
                }
            },
        };
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
                return Ok(());
            }
        }
    };

    info!("Matched '{}' -> '{}'", raw_title, tmdb_media.name);

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
    Ok(())
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
