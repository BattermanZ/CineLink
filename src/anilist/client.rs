use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use super::AniListMapped;

const ANILIST_ENDPOINT: &str = "https://graphql.anilist.co";

#[derive(Debug, Clone)]
pub struct AniListClient {
    client: Client,
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
        Ok(Self { client })
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
            .client
            .post(ANILIST_ENDPOINT)
            .json(&body)
            .send()
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

        let normalized = query.trim().to_ascii_lowercase();
        let best = hits.iter().find(|m| {
            m.title.as_ref().is_some_and(|t| {
                t.english
                    .as_deref()
                    .is_some_and(|s| s.trim().to_ascii_lowercase() == normalized)
                    || t.romaji
                        .as_deref()
                        .is_some_and(|s| s.trim().to_ascii_lowercase() == normalized)
            })
        });

        best.or_else(|| hits.first())
            .map(|m| m.id)
            .ok_or_else(|| anyhow!("No AniList match found for '{}'", query))
    }

    pub(crate) async fn fetch_relations(
        &self,
        media_type: AniListMediaType,
        id: i32,
    ) -> Result<Vec<RelationEdge>> {
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
    relations {
      edges {
        relationType
        node { id }
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
            .client
            .post(ANILIST_ENDPOINT)
            .json(&body)
            .send()
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

        Ok(parsed
            .data
            .and_then(|d| d.media)
            .and_then(|m| m.relations)
            .and_then(|r| r.edges)
            .unwrap_or_default())
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
            .client
            .post(ANILIST_ENDPOINT)
            .json(&body)
            .send()
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
    relations: Option<RelationsConnection>,
}

#[derive(Debug, Deserialize)]
struct RelationsConnection {
    edges: Option<Vec<RelationEdge>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RelationEdge {
    #[serde(rename = "relationType")]
    pub(crate) relation_type: Option<String>,
    pub(crate) node: Option<RelationNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RelationNode {
    pub(crate) id: i32,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MediaTitle {
    pub(crate) romaji: Option<String>,
    pub(crate) english: Option<String>,
}

#[derive(Debug, Deserialize)]
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
