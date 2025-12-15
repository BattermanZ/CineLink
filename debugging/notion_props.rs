//! Fetch Notion database properties and print each as pretty JSON on its own line.
//! Uses NOTION_API_KEY and NOTION_DATABASE_ID from the environment (.env supported).

use anyhow::{Context, Result};
use dotenvy::dotenv;
use reqwest::Client;
use serde_json::{Map, Value};
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env if present for local runs.
    dotenv().ok();

    let notion_api_key = env::var("NOTION_API_KEY")
        .context("Missing NOTION_API_KEY in environment")?;
    let notion_database_id = env::var("NOTION_DATABASE_ID")
        .context("Missing NOTION_DATABASE_ID in environment")?;

    let client = Client::new();
    let url = format!("https://api.notion.com/v1/databases/{}", notion_database_id);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", notion_api_key))
        .header("Notion-Version", "2022-06-28")
        .send()
        .await
        .context("Failed to call Notion API")?
        .error_for_status()
        .context("Notion API returned an error status")?;

    let body: Value = response.json().await.context("Failed to parse Notion response")?;
    let properties = body
        .get("properties")
        .and_then(|v| v.as_object())
        .context("No properties object found in Notion response")?;

    for name in properties.keys() {
        println!("{}", name);
    }

    Ok(())
}
