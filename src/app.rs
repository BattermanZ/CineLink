use crate::notion::{self, NotionApi, NotionClient};
use crate::notion_fallback::fallback_schema;
use crate::tmdb::{self, TmdbApi, TmdbClient};
use anyhow::Result;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct AppState {
    pub notion: Arc<dyn NotionApi>,
    pub tmdb: Arc<dyn TmdbApi>,
    pub title_property: String,
    pub schema: Arc<notion::PropertySchema>,
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

    let state = AppState {
        notion,
        tmdb,
        title_property,
        schema,
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
    Json(payload): Json<serde_json::Value>,
) -> StatusCode {
    if payload.get("type").and_then(|v| v.as_str()) != Some("page.properties_updated") {
        return StatusCode::OK;
    }

    let updated_raw = payload
        .get("data")
        .and_then(|d| d.get("updated_properties"))
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();

    let updated = updated_raw
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();

    let should_process = updated.iter().any(|p| {
        let lower = p.to_lowercase();
        lower == "title" || lower == "season"
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

    let raw_title = notion::extract_title(props, &state.title_property).unwrap_or_default();

    if !raw_title.ends_with(';') {
        return Ok(());
    }

    let clean_title = raw_title.trim_end_matches(';').trim().to_string();

    let type_value = notion::extract_select(props, "Type");
    let is_tv = type_value
        .as_deref()
        .map(|t| t.to_lowercase().contains("tv"))
        .unwrap_or(false);

    let season_str = notion::extract_select(props, "Season")
        .or_else(|| notion::extract_rich_text(props, "Season"));
    let season_number = if is_tv {
        match season_str.as_deref().and_then(tmdb::parse_season_number) {
            Some(n) => Some(n),
            None => {
                warn!("TV item missing or invalid season, skipping");
                return Ok(());
            }
        }
    } else {
        None
    };

    let tmdb_id = notion::extract_number(props, "ID")
        .map(|n| n as i32)
        .or_else(|| notion::extract_rich_text(props, "ID").and_then(|s| s.parse().ok()));

    let tmdb_media = if is_tv {
        let season = season_number.expect("season required");
        let show_id = match tmdb_id {
            Some(id) => id,
            None => state.tmdb.search_tv(&clean_title).await?,
        };
        state.tmdb.fetch_tv_season(show_id, season).await?
    } else {
        let movie_id = match tmdb_id {
            Some(id) => id,
            None => state.tmdb.search_movie(&clean_title).await?,
        };
        state.tmdb.fetch_movie(movie_id).await?
    };

    // Merge schema from live page properties (handles cases where DB schema is unavailable).
    let mut schema = (*state.schema).clone();
    notion::merge_schema_from_props(&mut schema, props);

    let mut updates = serde_json::Map::new();
    notion::set_title(
        &mut updates,
        &state.title_property,
        &tmdb_media.name,
        &schema,
    );

    notion::set_value(
        &mut updates,
        "Eng Name",
        Some(notion::ValueInput::Text(tmdb_media.eng_name)),
        &schema,
    );
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
        tmdb_media.poster.map(notion::ValueInput::Url),
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

    state.notion.update_page(page_id, updates).await?;
    Ok(())
}
