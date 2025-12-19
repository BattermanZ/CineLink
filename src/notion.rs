use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::env;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::{info, warn};

use crate::notion_fallback::fallback_schema;

pub const NOTION_VERSION: &str = "2025-09-03";
const MAX_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct NotionClient {
    client: Client,
    api_key: String,
    pub database_id: String,
    data_source_id: OnceCell<String>,
}

#[derive(Debug, Deserialize)]
struct NotionErrorBody {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug)]
struct NotionApiError {
    status: reqwest::StatusCode,
    code: Option<String>,
    message: Option<String>,
    raw: String,
}

impl std::fmt::Display for NotionApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code.as_deref(), self.message.as_deref()) {
            (Some(code), Some(msg)) => write!(f, "Notion API error {}: {}", code, msg),
            (Some(code), None) => write!(f, "Notion API error {}", code),
            (None, Some(msg)) => write!(f, "Notion API error: {}", msg),
            (None, None) => write!(f, "Notion API error (status {})", self.status),
        }?;
        if self.message.is_none() {
            write!(f, " (raw: {})", self.raw)?;
        }
        Ok(())
    }
}

impl std::error::Error for NotionApiError {}

#[derive(Debug, Deserialize)]
pub struct DatabaseQueryResponse {
    pub results: Vec<Value>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[async_trait]
pub trait NotionApi: Send + Sync {
    async fn fetch_property_schema(&self) -> Result<PropertySchema>;
    async fn fetch_page(&self, page_id: &str) -> Result<Value>;
    async fn update_page(
        &self,
        page_id: &str,
        properties: Map<String, Value>,
        icon: Option<Value>,
        cover: Option<Value>,
    ) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyType {
    Title,
    RichText,
    Url,
    Number,
    Select,
    MultiSelect,
    Files,
    Date,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct PropertySchema {
    pub types: HashMap<String, PropertyType>,
    pub title_property: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ValueInput {
    Text(String),
    StringList(Vec<String>),
    Number(f64),
    Url(String),
    Date(String),
}

impl NotionClient {
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("NOTION_API_KEY").context("NOTION_API_KEY not set")?;
        let database_id = env::var("NOTION_DATABASE_ID").context("NOTION_DATABASE_ID not set")?;
        let env_data_source_id = env::var("NOTION_DATA_SOURCE_ID")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let user_agent = format!("cinelink/{}", env!("CARGO_PKG_VERSION"));
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .user_agent(user_agent)
            .build()
            .context("Failed to build Notion HTTP client")?;
        let data_source_id = OnceCell::new();
        if let Some(ds) = env_data_source_id {
            let _ = data_source_id.set(ds);
        }
        Ok(Self {
            client,
            api_key,
            database_id,
            data_source_id,
        })
    }

    async fn send_with_retry(
        &self,
        mut make_req: impl FnMut() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        for attempt in 1..=MAX_RETRIES {
            let res = make_req().send().await;
            match res {
                Ok(resp) => {
                    let status = resp.status();
                    if (status.as_u16() == 429 || status.is_server_error()) && attempt < MAX_RETRIES
                    {
                        let delay = retry_delay(attempt, resp.headers().get("retry-after"));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect()) {
                        tokio::time::sleep(retry_delay(attempt, None)).await;
                        continue;
                    }
                    return Err(e).context("Notion request failed");
                }
            }
        }
        unreachable!("loop returns on success/final error")
    }

    async fn resolve_data_source_id(&self) -> Result<String> {
        if let Some(existing) = self.data_source_id.get() {
            return Ok(existing.clone());
        }

        let url = format!("https://api.notion.com/v1/databases/{}", self.database_id);
        let res = self
            .send_with_retry(|| {
                self.client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Notion-Version", NOTION_VERSION)
            })
            .await
            .context("Failed to fetch Notion database to resolve data source id")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read Notion database response body")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion database request failed (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        let body: Value =
            serde_json::from_slice(&bytes).context("Failed to parse database JSON")?;
        let ds_id = body
            .get("data_sources")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database response did not include data_sources id"))?;

        let _ = self.data_source_id.set(ds_id.clone());
        Ok(ds_id)
    }

    async fn post_query(
        &self,
        url: &str,
        start_cursor: Option<&str>,
        page_size: usize,
    ) -> Result<DatabaseQueryResponse> {
        let mut body = json!({ "page_size": page_size });
        if let Some(cursor) = start_cursor {
            body["start_cursor"] = Value::String(cursor.to_string());
        }

        let res = self
            .send_with_retry(|| {
                self.client
                    .post(url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Notion-Version", NOTION_VERSION)
                    .json(&body)
            })
            .await
            .context("Failed to query Notion database")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read Notion query response")?;
        if !status.is_success() {
            let raw = String::from_utf8_lossy(&bytes).into_owned();
            let parsed = serde_json::from_slice::<NotionErrorBody>(&bytes).ok();
            return Err(anyhow::Error::new(NotionApiError {
                status,
                code: parsed.as_ref().and_then(|p| p.code.clone()),
                message: parsed.as_ref().and_then(|p| p.message.clone()),
                raw,
            }));
        }

        serde_json::from_slice(&bytes).context("Failed to parse Notion query JSON")
    }

    pub async fn query_database_page(
        &self,
        start_cursor: Option<&str>,
        page_size: usize,
    ) -> Result<DatabaseQueryResponse> {
        if let Some(ds_id) = self.data_source_id.get() {
            let url_ds = format!("https://api.notion.com/v1/data_sources/{}/query", ds_id);
            return self.post_query(&url_ds, start_cursor, page_size).await;
        }

        let url_db = format!(
            "https://api.notion.com/v1/databases/{}/query",
            self.database_id
        );
        match self.post_query(&url_db, start_cursor, page_size).await {
            Ok(r) => Ok(r),
            Err(e) => {
                let is_invalid_request_url = e
                    .downcast_ref::<NotionApiError>()
                    .and_then(|err| err.code.as_deref())
                    == Some("invalid_request_url");

                if is_invalid_request_url {
                    let ds_id = self.resolve_data_source_id().await?;
                    let url_ds = format!("https://api.notion.com/v1/data_sources/{}/query", ds_id);
                    info!("Database query endpoint rejected; using data source query endpoint");
                    return self.post_query(&url_ds, start_cursor, page_size).await;
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl NotionApi for NotionClient {
    async fn fetch_property_schema(&self) -> Result<PropertySchema> {
        let url = format!("https://api.notion.com/v1/databases/{}", self.database_id);
        let res = self
            .send_with_retry(|| {
                self.client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Notion-Version", NOTION_VERSION)
            })
            .await
            .context("Failed to fetch Notion database")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read Notion response body")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion database request failed (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        let body: Value =
            serde_json::from_slice(&bytes).context("Failed to parse database JSON")?;
        if let Some(props) = body.get("properties").and_then(|p| p.as_object()) {
            return Ok(schema_from_properties(props));
        }

        // Fallback: query first page to infer property types (Notion 2025-09-03 may omit properties for synced DBs).
        match self.infer_schema_via_query().await {
            Ok(inferred) => Ok(inferred),
            Err(e) => {
                warn!(
                    "Failed to infer schema via query: {}. Using fallback schema.",
                    e
                );
                Ok(fallback_schema())
            }
        }
    }

    async fn fetch_page(&self, page_id: &str) -> Result<Value> {
        let url = format!("https://api.notion.com/v1/pages/{}", page_id);
        let res = self
            .send_with_retry(|| {
                self.client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Notion-Version", NOTION_VERSION)
            })
            .await
            .context("Failed to fetch Notion page")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read Notion page response")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion page request failed (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        serde_json::from_slice(&bytes).context("Failed to parse page JSON")
    }

    async fn update_page(
        &self,
        page_id: &str,
        properties: Map<String, Value>,
        icon: Option<Value>,
        cover: Option<Value>,
    ) -> Result<()> {
        let url = format!("https://api.notion.com/v1/pages/{}", page_id);
        let mut body = json!({ "properties": properties });
        if let Some(icon_val) = icon {
            body["icon"] = icon_val;
        }
        if let Some(cover_val) = cover {
            body["cover"] = cover_val;
        }

        let res = self
            .send_with_retry(|| {
                self.client
                    .patch(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Notion-Version", NOTION_VERSION)
                    .json(&body)
            })
            .await
            .context("Failed to update Notion page")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read Notion update response")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion page update failed (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        Ok(())
    }
}

pub fn extract_title(props: &Map<String, Value>, name: &str) -> Option<String> {
    props
        .get(name)
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| {
            item.get("plain_text")
                .or_else(|| item.get("text")?.get("content"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn extract_select(props: &Map<String, Value>, name: &str) -> Option<String> {
    props
        .get(name)
        .and_then(|p| p.get("select"))
        .and_then(|s| s.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn extract_rich_text(props: &Map<String, Value>, name: &str) -> Option<String> {
    props
        .get(name)
        .and_then(|p| p.get("rich_text"))
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| {
            item.get("plain_text")
                .or_else(|| item.get("text")?.get("content"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn extract_number(props: &Map<String, Value>, name: &str) -> Option<f64> {
    props
        .get(name)
        .and_then(|p| p.get("number"))
        .and_then(|v| v.as_f64())
}

pub fn set_title(
    target: &mut Map<String, Value>,
    property: &str,
    value: &str,
    schema: &PropertySchema,
) {
    let prop_type = schema
        .types
        .get(property)
        .cloned()
        .unwrap_or(PropertyType::Title);
    if matches!(prop_type, PropertyType::Title) {
        target.insert(
            property.to_string(),
            json!({
                "title": [
                    { "text": { "content": value }}
                ]
            }),
        );
    } else {
        target.insert(
            property.to_string(),
            json!({
                "rich_text": [
                    { "text": { "content": value }}
                ]
            }),
        );
    }
}

pub fn merge_schema_from_props(schema: &mut PropertySchema, props: &Map<String, Value>) {
    for (name, prop) in props {
        if schema.types.contains_key(name) {
            continue;
        }
        if let Some(t) = prop.get("type").and_then(|v| v.as_str()) {
            let mapped = match t {
                "title" => PropertyType::Title,
                "rich_text" => PropertyType::RichText,
                "url" => PropertyType::Url,
                "number" => PropertyType::Number,
                "select" => PropertyType::Select,
                "multi_select" => PropertyType::MultiSelect,
                "files" => PropertyType::Files,
                "date" => PropertyType::Date,
                other => PropertyType::Unknown(other.to_string()),
            };
            if mapped == PropertyType::Title && schema.title_property.is_none() {
                schema.title_property = Some(name.clone());
            }
            schema.types.insert(name.clone(), mapped);
        }
    }
}

fn schema_from_properties(props: &Map<String, Value>) -> PropertySchema {
    let mut types = HashMap::new();
    let mut title_property = None;

    for (name, def) in props {
        if let Some(t) = def.get("type").and_then(|v| v.as_str()) {
            let mapped = match t {
                "title" => PropertyType::Title,
                "rich_text" => PropertyType::RichText,
                "url" => PropertyType::Url,
                "number" => PropertyType::Number,
                "select" => PropertyType::Select,
                "multi_select" => PropertyType::MultiSelect,
                "files" => PropertyType::Files,
                "date" => PropertyType::Date,
                other => PropertyType::Unknown(other.to_string()),
            };
            if mapped == PropertyType::Title {
                title_property = Some(name.clone());
            }
            types.insert(name.clone(), mapped);
        }
    }

    PropertySchema {
        types,
        title_property,
    }
}

impl NotionClient {
    async fn infer_schema_via_query(&self) -> Result<PropertySchema> {
        let resp = self.query_database_page(None, 1).await?;
        let props = resp
            .results
            .first()
            .and_then(|first| first.get("properties"))
            .and_then(|p| p.as_object())
            .ok_or_else(|| anyhow::anyhow!("No properties found in Notion query response"))?;
        Ok(schema_from_properties(props))
    }
}
pub fn set_value(
    target: &mut Map<String, Value>,
    property: &str,
    value: Option<ValueInput>,
    schema: &PropertySchema,
) {
    let Some(val) = value else {
        return;
    };
    let prop_type = schema
        .types
        .get(property)
        .cloned()
        .unwrap_or(PropertyType::RichText);

    let payload = match prop_type {
        PropertyType::Title => Some(json!({
            "title": [
                { "text": { "content": string_value(val.clone()) } }
            ]
        })),
        PropertyType::RichText | PropertyType::Unknown(_) => Some(json!({
            "rich_text": [
                { "text": { "content": string_value(val) } }
            ]
        })),
        PropertyType::Url => string_value_opt(val).map(|s| json!({ "url": s })),
        PropertyType::Number => match val {
            ValueInput::Number(n) => Some(json!({ "number": n })),
            _ => None,
        },
        PropertyType::Select => string_value_opt(val).map(|s| json!({ "select": { "name": s } })),
        PropertyType::MultiSelect => Some(json!({
            "multi_select": match val {
                ValueInput::StringList(list) => list.into_iter().map(|n| json!({ "name": n })).collect::<Vec<_>>(),
                other => vec![json!({ "name": string_value(other) })],
            }
        })),
        PropertyType::Files => string_value_opt(val).map(|s| {
            json!({
                "files": [{
                    "name": "external",
                    "type": "external",
                    "external": { "url": s }
                }]
            })
        }),
        PropertyType::Date => string_value_opt(val).map(|s| json!({ "date": { "start": s } })),
    };

    if let Some(p) = payload {
        target.insert(property.to_string(), p);
    }
}

fn string_value(val: ValueInput) -> String {
    match val {
        ValueInput::Text(s) => s,
        ValueInput::StringList(list) => list.join(", "),
        ValueInput::Number(n) => n.to_string(),
        ValueInput::Url(s) => s,
        ValueInput::Date(s) => s,
    }
}

fn string_value_opt(val: ValueInput) -> Option<String> {
    match val {
        ValueInput::Text(s) => Some(s),
        ValueInput::StringList(list) => list.first().cloned(),
        ValueInput::Number(_) => None,
        ValueInput::Url(s) => Some(s),
        ValueInput::Date(s) => Some(s),
    }
}

fn retry_delay(attempt: usize, retry_after: Option<&reqwest::header::HeaderValue>) -> Duration {
    if let Some(v) = retry_after.and_then(|h| h.to_str().ok()) {
        if let Ok(secs) = v.parse::<u64>() {
            return Duration::from_secs(secs.min(30));
        }
    }
    let base_ms = 200u64.saturating_mul(2u64.saturating_pow((attempt - 1) as u32));
    let jitter_ms = jitter_ms();
    Duration::from_millis((base_ms + jitter_ms).min(5_000))
}

fn jitter_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (now.subsec_millis() as u64) % 100
}
