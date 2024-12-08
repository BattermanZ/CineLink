use anyhow::{Result, Context};
use axum::{
    routing::post,
    Router,
    http::StatusCode,
    response::IntoResponse,
    Json,
    extract::{State, Query, Multipart},
};
use log::{debug, info, warn, error};
use reqwest::Client;
use serde_json::json;
use std::env;
use std::net::SocketAddr;
use serde::Deserialize;

use crate::sync::run_bidirectional_sync;

#[derive(Deserialize)]
struct WebhookQuery {}

async fn sync_handler(headers: axum::http::HeaderMap, State(api_key): State<String>) -> impl IntoResponse {
    debug!("Sync request received");

    // Check if the API key is present and correct
    let auth_header = headers.get("Authorization");
    match auth_header {
        Some(header) if header == &format!("Bearer {}", api_key) => {
            debug!("Sync request received with valid API key");
        }
        _ => {
            error!("Sync request received with invalid or missing API key");
            return (StatusCode::UNAUTHORIZED, Json(json!({"status": "error", "message": "Invalid or missing API key"})));
        }
    }

    trigger_sync().await
}

async fn webhook_handler(
    Query(_params): Query<WebhookQuery>,
    State(_api_key): State<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut payload = String::new();

    while let Some(field) = multipart.next_field().await.unwrap() {
        let name = field.name().unwrap().to_string();
        if name == "payload" {
            payload = field.text().await.unwrap();
            break;
        }
    }

    if payload.is_empty() {
        error!("No payload found in the multipart form data");
        return (StatusCode::BAD_REQUEST, Json(json!({"status": "error", "message": "No payload found"})));
    }

    // Parse the payload
    let webhook_data: serde_json::Value = match serde_json::from_str(&payload) {
        Ok(data) => data,
        Err(e) => {
            error!("Failed to parse webhook payload: {}", e);
            return (StatusCode::BAD_REQUEST, Json(json!({"status": "error", "message": "Invalid payload format"})));
        }
    };

    let event = webhook_data["event"].as_str().unwrap_or("unknown");
    let title = webhook_data["Metadata"]["title"].as_str().unwrap_or("unknown");
    info!("Received webhook: event={}, title={}", event, title);

    if event == "media.rate" {
        debug!("Triggering sync for media.rate event");
        trigger_sync().await
    } else {
        debug!("Ignoring non-media.rate webhook");
        (StatusCode::OK, Json(json!({"status": "success", "message": "Webhook received but not processed"})))
    }
}

async fn trigger_sync() -> (StatusCode, Json<serde_json::Value>) {
    let client = Client::new();
    let plex_url = env::var("PLEX_URL").expect("PLEX_URL must be set");
    let plex_token = env::var("PLEX_TOKEN").expect("PLEX_TOKEN must be set");
    let notion_api_key = env::var("NOTION_API_KEY").expect("NOTION_API_KEY must be set");
    let notion_database_id = env::var("NOTION_DATABASE_ID").expect("NOTION_DATABASE_ID must be set");
    let notion_url = "https://api.notion.com/v1/pages";

    let mut notion_headers = reqwest::header::HeaderMap::new();
    notion_headers.insert("Authorization", format!("Bearer {}", notion_api_key).parse().unwrap());
    notion_headers.insert("Content-Type", "application/json".parse().unwrap());
    notion_headers.insert("Notion-Version", "2022-06-28".parse().unwrap());

    match run_bidirectional_sync(&client, &client, &plex_url, &plex_token, notion_url, &notion_headers, &notion_database_id).await {
        Ok(_) => {
            info!("Sync completed successfully");
            (StatusCode::OK, Json(json!({"status": "success", "message": "Sync completed successfully"})))
        },
        Err(e) => {
            error!("Sync failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"status": "error", "message": format!("Sync failed: {}", e)})))
        }
    }
}

pub async fn start_server() -> Result<()> {
    let api_key = env::var("API_KEY").expect("API_KEY must be set");
    info!("Starting server");

    let app = Router::new()
        .route("/sync", post(sync_handler))
        .route("/webhook", post(webhook_handler))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(api_key);

    let addr = SocketAddr::from(([0, 0, 0, 0], 9999));
    info!("Server listening on {}", addr);

    axum::serve(tokio::net::TcpListener::bind(addr).await?, app)
        .await
        .context("Failed to start server")?;

    Ok(())
}

