use anyhow::Result;
use cinelink::app::{process_page_backfill_tv, AppState, WindowCounter};
use cinelink::notion::{self, DatabaseQueryResponse, NotionApi, NotionClient};
use cinelink::notion_fallback::fallback_schema;
use cinelink::tmdb::{self, TmdbApi, TmdbClient};
use dotenvy::dotenv;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn parse_concurrency() -> usize {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--concurrency" {
            if let Some(v) = args.next() {
                if let Ok(n) = v.parse::<usize>() {
                    return n.clamp(1, 64);
                }
            }
        }
    }
    8
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv();
    init_tracing();

    let concurrency = parse_concurrency();
    info!("Starting TV backfill (concurrency={})", concurrency);

    let notion_client = NotionClient::from_env()?;
    let notion: Arc<dyn NotionApi> = Arc::new(notion_client.clone());
    let schema = match notion.fetch_property_schema().await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            warn!("Failed to fetch Notion schema, using fallback: {}", e);
            Arc::new(fallback_schema())
        }
    };
    let title_property = schema
        .title_property
        .clone()
        .unwrap_or_else(|| "Name".to_string());
    let tmdb: Arc<dyn TmdbApi> = Arc::new(TmdbClient::from_env()?);

    let state = AppState {
        notion,
        tmdb,
        title_property,
        schema,
        signing_secret: String::new(),
        rate_limits: Arc::new(Mutex::new(HashMap::new())),
        global_limit: Arc::new(Mutex::new(WindowCounter {
            window: 0,
            count: 0,
        })),
        recent_events: Arc::new(Mutex::new(HashMap::new())),
        processing_sem: Arc::new(Semaphore::new(concurrency)),
    };

    let sem = Arc::new(Semaphore::new(concurrency));
    let mut joinset = JoinSet::new();
    let mut cursor: Option<String> = None;

    let mut scanned = 0usize;
    let mut candidates = 0usize;
    let mut updated = 0usize;

    loop {
        let DatabaseQueryResponse {
            results,
            has_more,
            next_cursor,
        } = notion_client
            .query_database_page(cursor.as_deref(), 100)
            .await?;

        for page in results {
            scanned += 1;
            let Some(page_id) = page.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
                continue;
            };
            let Some(props) = page.get("properties").and_then(|p| p.as_object()).cloned() else {
                continue;
            };

            let title = notion::extract_title(&props, &state.title_property).unwrap_or_default();
            if title.trim().is_empty() || title.ends_with(';') {
                continue;
            }

            let type_value = notion::extract_select(&props, "Type").unwrap_or_default();
            if !type_value.to_lowercase().contains("tv") {
                continue;
            }

            let season = notion::extract_select(&props, "Season");
            if season
                .as_deref()
                .and_then(tmdb::parse_season_number)
                .is_none()
            {
                continue;
            }

            candidates += 1;
            let state_for_task = state.clone();
            let sem_for_task = sem.clone();
            joinset.spawn(async move {
                let _permit = sem_for_task.acquire_owned().await?;
                process_page_backfill_tv(&state_for_task, &page_id).await
            });

            while joinset.len() >= concurrency * 4 {
                if let Some(res) = joinset.join_next().await {
                    match res {
                        Ok(Ok(true)) => updated += 1,
                        Ok(Ok(false)) => {}
                        Ok(Err(e)) => error!("Backfill task failed: {}", e),
                        Err(e) => error!("Backfill task panicked: {}", e),
                    }
                }
            }
        }

        if !has_more {
            break;
        }
        cursor = next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    while let Some(res) = joinset.join_next().await {
        match res {
            Ok(Ok(true)) => updated += 1,
            Ok(Ok(false)) => {}
            Ok(Err(e)) => error!("Backfill task failed: {}", e),
            Err(e) => error!("Backfill task panicked: {}", e),
        }
    }

    info!(
        "TV backfill complete: scanned {} pages, matched {} candidates, updated {} series",
        scanned, candidates, updated
    );
    Ok(())
}
