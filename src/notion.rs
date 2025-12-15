use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::env;
use tracing::warn;

use crate::notion_fallback::fallback_schema;

const NOTION_VERSION: &str = "2025-09-03";

#[derive(Debug, Clone)]
pub struct NotionClient {
    client: Client,
    api_key: String,
    pub database_id: String,
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
        Ok(Self {
            client: Client::new(),
            api_key,
            database_id,
        })
    }
}

#[async_trait]
impl NotionApi for NotionClient {
    async fn fetch_property_schema(&self) -> Result<PropertySchema> {
        let url = format!("https://api.notion.com/v1/databases/{}", self.database_id);
        let res = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Notion-Version", NOTION_VERSION)
            .send()
            .await
            .context("Failed to fetch Notion database")?;

        let status = res.status();
        let body_text = res
            .text()
            .await
            .context("Failed to read Notion response body")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion database request failed (status {}): {}",
                status,
                body_text
            ));
        }

        let body: Value =
            serde_json::from_str(&body_text).context("Failed to parse database JSON")?;
        if let Some(props) = body.get("properties").and_then(|p| p.as_object()) {
            return Ok(schema_from_properties(props));
        }

        // Fallback: query first page to infer property types (Notion 2025-09-03 may omit properties for synced DBs).
        match fetch_schema_via_query(&self.client, &self.api_key, &self.database_id).await {
            Ok(inferred) => Ok(inferred),
            Err(e) => {
                warn!("Failed to infer schema via query: {}. Using fallback schema.", e);
                Ok(fallback_schema())
            }
        }
    }

    async fn fetch_page(&self, page_id: &str) -> Result<Value> {
        let url = format!("https://api.notion.com/v1/pages/{}", page_id);
        let res = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Notion-Version", NOTION_VERSION)
            .send()
            .await
            .context("Failed to fetch Notion page")?;

        let status = res.status();
        let text = res
            .text()
            .await
            .context("Failed to read Notion page response")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion page request failed (status {}): {}",
                status,
                text
            ));
        }

        serde_json::from_str(&text).context("Failed to parse page JSON")
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
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Notion-Version", NOTION_VERSION)
            .json(&body)
            .send()
            .await
            .context("Failed to update Notion page")?;

        let status = res.status();
        let text = res
            .text()
            .await
            .context("Failed to read Notion update response")?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Notion page update failed (status {}): {}",
                status,
                text
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

async fn fetch_schema_via_query(
    client: &Client,
    api_key: &str,
    database_id: &str,
) -> Result<PropertySchema> {
    let url = format!("https://api.notion.com/v1/databases/{}/query", database_id);
    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Notion-Version", NOTION_VERSION)
        .json(&json!({ "page_size": 1 }))
        .send()
        .await
        .context("Failed to query Notion database for schema inference")?;

    let status = res.status();
    let text = res
        .text()
        .await
        .context("Failed to read Notion query response")?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "Notion query failed (status {}): {}",
            status,
            text
        ));
    }

    let body: Value = serde_json::from_str(&text).context("Failed to parse Notion query JSON")?;
    let props = body
        .get("results")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("properties"))
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow::anyhow!("No properties found in Notion query response"))?;

    Ok(schema_from_properties(props))
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
