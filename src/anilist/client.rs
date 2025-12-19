use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Mutex;

use super::AniListMapped;

const ANILIST_ENDPOINT: &str = "https://graphql.anilist.co";
const RELATIONS_CACHE_TTL_SECS: u64 = 60 * 60 * 24; // 24 hours
const TITLE_CACHE_TTL_SECS: u64 = 60 * 60 * 24; // 24 hours
const MAX_CACHE_ENTRIES: usize = 20_000;
const MAX_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct AniListClient {
    client: Client,
    relations_cache: Arc<Mutex<HashMap<i32, CacheEntry<RelationsPayload>>>>,
    title_cache: Arc<Mutex<HashMap<i32, CacheEntry<MediaTitle>>>>,
}

#[derive(Debug, Clone)]
struct CacheEntry<T> {
    inserted_at: Instant,
    value: T,
}

#[derive(Debug, Clone, Copy)]
pub enum AniListMediaType {
    Anime,
    Manga,
}

impl AniListMediaType {
    fn as_graphql(&self) -> &'static str {
        match self {
            AniListMediaType::Anime => "ANIME",
            AniListMediaType::Manga => "MANGA",
        }
    }
}

impl AniListClient {
    pub fn new() -> Result<Self> {
        let user_agent = format!("cinelink/{}", env!("CARGO_PKG_VERSION"));
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .user_agent(user_agent)
            .build()
            .context("Failed to build AniList HTTP client")?;
        Ok(Self {
            client,
            relations_cache: Arc::new(Mutex::new(HashMap::new())),
            title_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn fetch_mapped(
        &self,
        media_type: AniListMediaType,
        id: i32,
    ) -> Result<AniListMapped> {
        let media = self.fetch_media(media_type, id).await?;
        self.map_media(media_type, media).await
    }

    pub(crate) async fn search_id(&self, media_type: AniListMediaType, query: &str) -> Result<i32> {
        let candidates = self.search_candidates(media_type, query).await?;
        let normalized = normalize_title_key(query);
        let best = candidates
            .iter()
            .find(|m| {
                m.english
                    .as_deref()
                    .is_some_and(|s| normalize_title_key(s) == normalized)
                    || m.romaji
                        .as_deref()
                        .is_some_and(|s| normalize_title_key(s) == normalized)
            })
            .or_else(|| candidates.first())
            .ok_or_else(|| anyhow!("No AniList match found for '{}'", query))?;
        Ok(best.id)
    }

    pub(crate) async fn search_candidates(
        &self,
        media_type: AniListMediaType,
        query: &str,
    ) -> Result<Vec<SearchCandidate>> {
        #[derive(Deserialize)]
        struct GraphQlResponse<T> {
            data: Option<T>,
            errors: Option<Vec<GraphQlError>>,
        }

        #[derive(Deserialize)]
        struct GraphQlError {
            message: String,
            status: Option<i32>,
        }

        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "Page")]
            page: Option<SearchPage>,
        }

        #[derive(Deserialize)]
        struct SearchPage {
            media: Option<Vec<SearchMedia>>,
        }

        #[derive(Deserialize)]
        struct SearchMedia {
            id: i32,
            title: Option<MediaTitle>,
        }

        let query_gql = r#"
query ($search: String!, $type: MediaType!) {
  Page(perPage: 5) {
    media(search: $search, type: $type) {
      id
      title { romaji english }
    }
  }
}
"#;

        let body = json!({
            "query": query_gql,
            "variables": { "search": query, "type": media_type.as_graphql() }
        });

        let res = self
            .post_with_retry(|| self.client.post(ANILIST_ENDPOINT).json(&body))
            .await
            .context("AniList search request failed")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read AniList search body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "AniList search HTTP error (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        let parsed: GraphQlResponse<Data> =
            serde_json::from_slice(&bytes).context("Failed to parse AniList search JSON")?;
        if let Some(errors) = parsed.errors {
            let msg = errors
                .into_iter()
                .map(|e| match e.status {
                    Some(s) => format!("{} (status {})", e.message, s),
                    None => e.message,
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!("AniList search GraphQL error: {}", msg));
        }

        let hits = parsed
            .data
            .and_then(|d| d.page)
            .and_then(|p| p.media)
            .unwrap_or_default();

        Ok(hits
            .into_iter()
            .map(|m| {
                let t = m.title.unwrap_or_default();
                SearchCandidate {
                    id: m.id,
                    english: t.english,
                    romaji: t.romaji,
                }
            })
            .collect())
    }

    pub(crate) async fn fetch_relations(
        &self,
        media_type: AniListMediaType,
        id: i32,
    ) -> Result<RelationsPayload> {
        if let Some(cached) = self.get_cached_relations(id).await {
            return Ok(cached);
        }

        #[derive(Deserialize)]
        struct GraphQlResponse<T> {
            data: Option<T>,
            errors: Option<Vec<GraphQlError>>,
        }

        #[derive(Deserialize)]
        struct GraphQlError {
            message: String,
            status: Option<i32>,
        }

        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "Media")]
            media: Option<MediaRelations>,
        }

        let query = r#"
query ($id: Int!, $type: MediaType!) {
  Media(id: $id, type: $type) {
    startDate { year month day }
    relations {
      edges {
        relationType
        node {
          id
          startDate { year month day }
        }
      }
    }
  }
}
"#;

        let body = json!({
            "query": query,
            "variables": { "id": id, "type": media_type.as_graphql() }
        });

        let res = self
            .post_with_retry(|| self.client.post(ANILIST_ENDPOINT).json(&body))
            .await
            .context("AniList relations request failed")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read AniList relations body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "AniList relations HTTP error (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        let parsed: GraphQlResponse<Data> =
            serde_json::from_slice(&bytes).context("Failed to parse AniList relations JSON")?;
        if let Some(errors) = parsed.errors {
            let msg = errors
                .into_iter()
                .map(|e| match e.status {
                    Some(s) => format!("{} (status {})", e.message, s),
                    None => e.message,
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!("AniList relations GraphQL error: {}", msg));
        }

        let payload = RelationsPayload {
            start_date: parsed
                .data
                .as_ref()
                .and_then(|d| d.media.as_ref())
                .and_then(|m| m.start_date.clone()),
            edges: parsed
                .data
                .and_then(|d| d.media)
                .and_then(|m| m.relations)
                .and_then(|r| r.edges)
                .unwrap_or_default(),
        };
        self.put_cached_relations(id, payload.clone()).await;
        Ok(payload)
    }

    pub(crate) async fn fetch_media(&self, media_type: AniListMediaType, id: i32) -> Result<Media> {
        #[derive(Deserialize)]
        struct GraphQlResponse<T> {
            data: Option<T>,
            errors: Option<Vec<GraphQlError>>,
        }

        #[derive(Deserialize)]
        struct GraphQlError {
            message: String,
            status: Option<i32>,
        }

        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "Media")]
            media: Option<Media>,
        }

        // Keep this query stable and explicit; it’s intended for human inspection/logging.
        let query = r#"
query ($id: Int!, $type: MediaType!) {
  Media(id: $id, type: $type) {
    id
    idMal
    siteUrl
    title { romaji english }
    description(asHtml: false)
    format
    status
    episodes
    duration
    countryOfOrigin
    isAdult
    genres
    startDate { year month day }
    coverImage { extraLarge }
    bannerImage
    trailer { id site thumbnail }
    characters(perPage: 10, sort: [ROLE]) { edges { node { name { full } } } }
    staff(perPage: 50) { edges { role node { name { full } } } }
  }
}
"#;

        let body = json!({
            "query": query,
            "variables": { "id": id, "type": media_type.as_graphql() }
        });

        let res = self
            .post_with_retry(|| self.client.post(ANILIST_ENDPOINT).json(&body))
            .await
            .context("AniList request failed")?;

        let status = res.status();
        let bytes = res.bytes().await.context("Failed to read AniList body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "AniList HTTP error (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        let parsed: GraphQlResponse<Data> =
            serde_json::from_slice(&bytes).context("Failed to parse AniList JSON")?;
        if let Some(errors) = parsed.errors {
            let msg = errors
                .into_iter()
                .map(|e| match e.status {
                    Some(s) => format!("{} (status {})", e.message, s),
                    None => e.message,
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!("AniList GraphQL error: {}", msg));
        }

        parsed
            .data
            .and_then(|d| d.media)
            .ok_or_else(|| anyhow!("AniList returned no media for id {}", id))
    }

    pub(crate) async fn fetch_titles(
        &self,
        media_type: AniListMediaType,
        id: i32,
    ) -> Result<MediaTitle> {
        if let Some(cached) = self.get_cached_title(id).await {
            return Ok(cached);
        }

        #[derive(Deserialize)]
        struct GraphQlResponse<T> {
            data: Option<T>,
            errors: Option<Vec<GraphQlError>>,
        }

        #[derive(Deserialize)]
        struct GraphQlError {
            message: String,
            status: Option<i32>,
        }

        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "Media")]
            media: Option<MediaTitleHolder>,
        }

        #[derive(Deserialize)]
        struct MediaTitleHolder {
            title: Option<MediaTitle>,
        }

        let query = r#"
query ($id: Int!, $type: MediaType!) {
  Media(id: $id, type: $type) { title { romaji english } }
}
"#;

        let body = json!({
            "query": query,
            "variables": { "id": id, "type": media_type.as_graphql() }
        });

        let res = self
            .post_with_retry(|| self.client.post(ANILIST_ENDPOINT).json(&body))
            .await
            .context("AniList titles request failed")?;

        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .context("Failed to read AniList titles body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "AniList titles HTTP error (status {}): {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }

        let parsed: GraphQlResponse<Data> =
            serde_json::from_slice(&bytes).context("Failed to parse AniList titles JSON")?;
        if let Some(errors) = parsed.errors {
            let msg = errors
                .into_iter()
                .map(|e| match e.status {
                    Some(s) => format!("{} (status {})", e.message, s),
                    None => e.message,
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!("AniList titles GraphQL error: {}", msg));
        }

        let title = parsed
            .data
            .and_then(|d| d.media)
            .and_then(|m| m.title)
            .unwrap_or_default();

        self.put_cached_title(id, title.clone()).await;
        Ok(title)
    }

    async fn post_with_retry(
        &self,
        mut make_req: impl FnMut() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let mut attempt = 0usize;
        loop {
            attempt += 1;
            let res = make_req().send().await;
            match res {
                Ok(resp) => {
                    if (resp.status().as_u16() == 429 || resp.status().is_server_error())
                        && attempt < MAX_RETRIES
                    {
                        let delay = retry_delay(attempt, resp.headers().get("retry-after"));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect() || e.is_request())
                    {
                        tokio::time::sleep(retry_delay(attempt, None)).await;
                        continue;
                    }
                    return Err(e).context("HTTP request failed");
                }
            }
        }
    }

    async fn get_cached_relations(&self, id: i32) -> Option<RelationsPayload> {
        let mut guard = self.relations_cache.lock().await;
        guard.retain(|_, v| v.inserted_at.elapsed().as_secs() < RELATIONS_CACHE_TTL_SECS);
        guard.get(&id).map(|e| e.value.clone())
    }

    async fn put_cached_relations(&self, id: i32, payload: RelationsPayload) {
        let mut guard = self.relations_cache.lock().await;
        if guard.len() > MAX_CACHE_ENTRIES {
            guard.clear();
        }
        guard.insert(
            id,
            CacheEntry {
                inserted_at: Instant::now(),
                value: payload,
            },
        );
    }

    async fn get_cached_title(&self, id: i32) -> Option<MediaTitle> {
        let mut guard = self.title_cache.lock().await;
        guard.retain(|_, v| v.inserted_at.elapsed().as_secs() < TITLE_CACHE_TTL_SECS);
        guard.get(&id).map(|e| e.value.clone())
    }

    async fn put_cached_title(&self, id: i32, title: MediaTitle) {
        let mut guard = self.title_cache.lock().await;
        if guard.len() > MAX_CACHE_ENTRIES {
            guard.clear();
        }
        guard.insert(
            id,
            CacheEntry {
                inserted_at: Instant::now(),
                value: title,
            },
        );
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Media {
    pub(crate) id: i32,
    #[serde(rename = "idMal")]
    pub(crate) id_mal: Option<i32>,
    #[serde(rename = "siteUrl")]
    pub(crate) site_url: Option<String>,
    pub(crate) title: Option<MediaTitle>,
    pub(crate) description: Option<String>,
    #[serde(rename = "countryOfOrigin")]
    pub(crate) country_of_origin: Option<String>,
    #[serde(rename = "isAdult")]
    pub(crate) is_adult: Option<bool>,
    pub(crate) genres: Option<Vec<String>>,
    #[serde(rename = "startDate")]
    pub(crate) start_date: Option<FuzzyDate>,
    pub(crate) episodes: Option<i32>,
    pub(crate) duration: Option<i32>,
    #[serde(rename = "coverImage")]
    pub(crate) cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage")]
    pub(crate) banner_image: Option<String>,
    pub(crate) trailer: Option<Trailer>,
    pub(crate) characters: Option<CharacterConnection>,
    pub(crate) staff: Option<StaffConnection>,
}

#[derive(Debug, Deserialize)]
struct MediaRelations {
    #[serde(rename = "startDate")]
    start_date: Option<FuzzyDate>,
    relations: Option<RelationsConnection>,
}

#[derive(Debug, Deserialize)]
struct RelationsConnection {
    edges: Option<Vec<RelationEdge>>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct RelationEdge {
    #[serde(rename = "relationType")]
    pub(crate) relation_type: Option<String>,
    pub(crate) node: Option<RelationNode>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct RelationNode {
    pub(crate) id: i32,
    #[serde(rename = "startDate")]
    pub(crate) start_date: Option<FuzzyDate>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub(crate) struct MediaTitle {
    pub(crate) romaji: Option<String>,
    pub(crate) english: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct FuzzyDate {
    pub(crate) year: Option<i32>,
    pub(crate) month: Option<i32>,
    pub(crate) day: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CoverImage {
    #[serde(rename = "extraLarge")]
    pub(crate) extra_large: Option<String>,
    pub(crate) large: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Trailer {
    pub(crate) id: Option<String>,
    pub(crate) site: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CharacterConnection {
    pub(crate) edges: Option<Vec<CharacterEdge>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CharacterEdge {
    pub(crate) node: Option<CharacterNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CharacterNode {
    pub(crate) name: Option<Name>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StaffConnection {
    pub(crate) edges: Option<Vec<StaffEdge>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StaffEdge {
    pub(crate) role: Option<String>,
    pub(crate) node: Option<StaffNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StaffNode {
    pub(crate) name: Option<Name>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Name {
    pub(crate) full: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchCandidate {
    pub(crate) id: i32,
    pub(crate) english: Option<String>,
    pub(crate) romaji: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RelationsPayload {
    pub(crate) start_date: Option<FuzzyDate>,
    pub(crate) edges: Vec<RelationEdge>,
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

fn normalize_title_key(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = false;
    for ch in input.chars() {
        let ch = ch.to_ascii_lowercase();
        let is_alnum = ch.is_ascii_alphanumeric();
        if is_alnum {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserializes_characters_field_for_cast() {
        let value = json!({
            "id": 1,
            "title": { "romaji": "Romaji", "english": "English" },
            "characters": {
                "edges": [
                    { "node": { "name": { "full": "Char A" } } },
                    { "node": { "name": { "full": "Char B" } } }
                ]
            }
        });
        let media: Media = serde_json::from_value(value).expect("media deserialize");
        let cast = media
            .characters
            .and_then(|c| c.edges)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| e.node.and_then(|n| n.name).and_then(|n| n.full))
            .collect::<Vec<_>>();
        assert_eq!(cast, vec!["Char A".to_string(), "Char B".to_string()]);
    }
}
