use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;

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

#[async_trait]
pub trait AniListApi: Send + Sync {
    async fn resolve_anime_id(&self, query: &str, season: Option<i32>) -> Result<i32>;
    async fn fetch_anime(&self, id: i32) -> Result<AniListMapped>;
}

#[derive(Debug, Clone)]
pub struct AniListMapped {
    pub id: i32,
    pub id_mal: Option<i32>,
    pub name: String,
    pub eng_name: Option<String>,
    pub original_title: Option<String>,
    pub synopsis: Option<String>,
    pub genres: Vec<String>,
    pub cast: Vec<String>,
    pub director: Vec<String>,
    pub is_adult: bool,
    pub content_rating: String,
    pub country_of_origin: Option<String>,
    pub language: Option<String>,
    pub release_date: Option<String>,
    pub year: Option<String>,
    pub runtime_minutes: Option<f32>,
    pub episodes: Option<i32>,
    pub trailer: Option<String>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub imdb_page: Option<String>,
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

    pub async fn resolve_id(&self, media_type: AniListMediaType, query: &str) -> Result<i32> {
        if let Some(id) = parse_anilist_id(query) {
            return Ok(id);
        }
        self.search_id(media_type, query).await
    }

    pub async fn resolve_id_with_season(
        &self,
        media_type: AniListMediaType,
        query: &str,
        season: Option<i32>,
    ) -> Result<i32> {
        if let Some(id) = parse_anilist_id(query) {
            return Ok(id);
        }
        let candidate = self.search_id(media_type, query).await?;
        let season = season.unwrap_or(1).max(1);
        self.resolve_season_entry(media_type, candidate, season)
            .await
    }

    async fn search_id(&self, media_type: AniListMediaType, query: &str) -> Result<i32> {
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

    async fn resolve_season_entry(
        &self,
        media_type: AniListMediaType,
        candidate_id: i32,
        season: i32,
    ) -> Result<i32> {
        let base = self.find_base_entry(media_type, candidate_id).await?;
        if season <= 1 {
            return Ok(base);
        }
        self.follow_sequel_chain(media_type, base, season - 1).await
    }

    async fn find_base_entry(&self, media_type: AniListMediaType, start_id: i32) -> Result<i32> {
        let mut current = start_id;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current) {
                return Ok(current);
            }
            let relations = self.fetch_relations(media_type, current).await?;
            let prequel = relations
                .iter()
                .find(|e| e.relation_type.as_deref() == Some("PREQUEL"))
                .and_then(|e| e.node.as_ref())
                .map(|n| n.id);
            match prequel {
                Some(prev) => current = prev,
                None => return Ok(current),
            }
        }
    }

    async fn follow_sequel_chain(
        &self,
        media_type: AniListMediaType,
        start_id: i32,
        steps: i32,
    ) -> Result<i32> {
        let mut current = start_id;
        let mut seen = HashSet::new();
        for _ in 0..steps {
            if !seen.insert(current) {
                break;
            }
            let relations = self.fetch_relations(media_type, current).await?;
            let sequel = relations
                .iter()
                .find(|e| e.relation_type.as_deref() == Some("SEQUEL"))
                .and_then(|e| e.node.as_ref())
                .map(|n| n.id)
                .ok_or_else(|| anyhow!("No AniList sequel found while resolving season"))?;
            current = sequel;
        }
        Ok(current)
    }

    async fn fetch_relations(
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

    async fn fetch_media(&self, media_type: AniListMediaType, id: i32) -> Result<Media> {
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

    async fn map_media(
        &self,
        _media_type: AniListMediaType,
        media: Media,
    ) -> Result<AniListMapped> {
        let title = media.title.unwrap_or_default();
        let (name, eng_name, original_title) = choose_titles(&title);

        let director = media
            .staff
            .and_then(|s| s.edges)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.role.as_deref().is_some_and(is_director_role))
            .filter_map(|e| e.node.and_then(|n| n.name).and_then(|n| n.full))
            .collect::<Vec<_>>();
        let director = dedupe_preserve_order(director);

        let cast = media
            .characters
            .and_then(|c| c.edges)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| e.node.and_then(|n| n.name).and_then(|n| n.full))
            .collect::<Vec<_>>();

        let country_code = media.country_of_origin.clone();
        let country_of_origin = country_code
            .as_deref()
            .map(|c| country_name_from_code(c).unwrap_or_else(|| c.to_string()));
        let language = country_code.as_deref().and_then(language_from_country);
        let is_adult = media.is_adult.unwrap_or(false);
        let content_rating = content_rating_from_is_adult(is_adult).to_string();

        let release_date = media.start_date.as_ref().and_then(fuzzy_date_to_string);
        let year = release_date
            .as_deref()
            .and_then(|d| d.split('-').next())
            .map(|s| s.to_string());

        let trailer = media.trailer.and_then(|t| trailer_url(&t));

        let poster = media
            .cover_image
            .as_ref()
            .and_then(|c| c.extra_large.clone())
            .or_else(|| media.cover_image.as_ref().and_then(|c| c.large.clone()));

        Ok(AniListMapped {
            id: media.id,
            id_mal: media.id_mal,
            name,
            eng_name,
            original_title,
            synopsis: media
                .description
                .as_deref()
                .map(clean_anilist_synopsis)
                .filter(|s| !s.trim().is_empty()),
            genres: media.genres.unwrap_or_default(),
            cast,
            director,
            is_adult,
            content_rating,
            country_of_origin,
            language,
            release_date,
            year,
            runtime_minutes: media.duration.map(|d| d as f32),
            episodes: media.episodes,
            trailer,
            poster,
            backdrop: media.banner_image,
            imdb_page: media.site_url,
        })
    }
}

#[derive(Debug, Deserialize)]
struct Media {
    id: i32,
    #[serde(rename = "idMal")]
    id_mal: Option<i32>,
    #[serde(rename = "siteUrl")]
    site_url: Option<String>,
    title: Option<MediaTitle>,
    description: Option<String>,
    #[serde(rename = "countryOfOrigin")]
    country_of_origin: Option<String>,
    #[serde(rename = "isAdult")]
    is_adult: Option<bool>,
    genres: Option<Vec<String>>,
    #[serde(rename = "startDate")]
    start_date: Option<FuzzyDate>,
    episodes: Option<i32>,
    duration: Option<i32>,
    #[serde(rename = "coverImage")]
    cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage")]
    banner_image: Option<String>,
    trailer: Option<Trailer>,
    characters: Option<CharacterConnection>,
    staff: Option<StaffConnection>,
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
struct RelationEdge {
    #[serde(rename = "relationType")]
    relation_type: Option<String>,
    node: Option<RelationNode>,
}

#[derive(Debug, Deserialize)]
struct RelationNode {
    id: i32,
}

#[derive(Debug, Default, Deserialize)]
struct MediaTitle {
    romaji: Option<String>,
    english: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FuzzyDate {
    year: Option<i32>,
    month: Option<i32>,
    day: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    #[serde(rename = "extraLarge")]
    extra_large: Option<String>,
    large: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Trailer {
    id: Option<String>,
    site: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CharacterConnection {
    edges: Option<Vec<CharacterEdge>>,
}

#[derive(Debug, Deserialize)]
struct CharacterEdge {
    node: Option<CharacterNode>,
}

#[derive(Debug, Deserialize)]
struct CharacterNode {
    name: Option<Name>,
}

#[derive(Debug, Deserialize)]
struct StaffConnection {
    edges: Option<Vec<StaffEdge>>,
}

#[derive(Debug, Deserialize)]
struct StaffEdge {
    role: Option<String>,
    node: Option<StaffNode>,
}

#[derive(Debug, Deserialize)]
struct StaffNode {
    name: Option<Name>,
}

#[derive(Debug, Deserialize)]
struct Name {
    full: Option<String>,
}

fn is_director_role(role: &str) -> bool {
    let role = role.to_ascii_lowercase();
    role.contains("director") && !role.contains("assistant director")
}

fn content_rating_from_is_adult(is_adult: bool) -> &'static str {
    if is_adult {
        "Adult"
    } else {
        "All Audiences"
    }
}

fn choose_titles(title: &MediaTitle) -> (String, Option<String>, Option<String>) {
    // For anime, we avoid non-Latin scripts here:
    // - Prefer English as the "actual" title (Eng Name / main title)
    // - Use romaji as the Original Title
    // - Do not populate Eng Name (it matches the title)
    let english = title
        .english
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let romaji = title
        .romaji
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let actual = english.or(romaji).unwrap_or("Unknown Title").to_string();
    let actual = strip_trailing_season_suffix(&actual);

    let eng_name = None;

    let original_title = romaji.map(strip_trailing_season_suffix);

    (actual, eng_name, original_title)
}

pub(crate) fn strip_trailing_season_suffix(title: &str) -> String {
    let trimmed = title.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    let Some(season_idx) = lower.rfind("season") else {
        return trimmed.to_string();
    };

    // Must be a trailing "season <digits>" (optionally preceded by whitespace/punctuation).
    let after = &lower[season_idx..];
    let after = after.strip_prefix("season").unwrap_or(after);
    let after = after.trim_start();
    if after.is_empty() || !after.bytes().all(|b| b.is_ascii_digit()) {
        return trimmed.to_string();
    }

    // Ensure "season" starts at a token boundary.
    if season_idx > 0 {
        let prev = lower.as_bytes()[season_idx - 1];
        if prev.is_ascii_alphanumeric() {
            return trimmed.to_string();
        }
    }

    // Compute cut position and also remove separators like " - ", ": ", " – ".
    let mut cut = season_idx;
    while cut > 0 && trimmed.as_bytes()[cut - 1].is_ascii_whitespace() {
        cut -= 1;
    }
    while cut > 0 {
        let ch = trimmed.as_bytes()[cut - 1] as char;
        if matches!(ch, '-' | ':' | '–' | '—') {
            cut -= 1;
            while cut > 0 && trimmed.as_bytes()[cut - 1].is_ascii_whitespace() {
                cut -= 1;
            }
        } else {
            break;
        }
    }

    let stripped = trimmed[..cut].trim_end();
    if stripped.is_empty() {
        trimmed.to_string()
    } else {
        stripped.to_string()
    }
}

fn parse_anilist_id(query: &str) -> Option<i32> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<i32>().ok().filter(|id| *id > 0)
}

fn fuzzy_date_to_string(date: &FuzzyDate) -> Option<String> {
    let year = date.year?;
    let month = date.month?;
    let day = date.day?;
    Some(format!("{:04}-{:02}-{:02}", year, month, day))
}

fn trailer_url(trailer: &Trailer) -> Option<String> {
    let site = trailer.site.as_deref()?;
    let id = trailer.id.as_deref()?;
    if site.eq_ignore_ascii_case("youtube") {
        return Some(format!("https://www.youtube.com/watch?v={}", id));
    }
    if site.eq_ignore_ascii_case("dailymotion") {
        return Some(format!("https://www.dailymotion.com/video/{}", id));
    }
    None
}

fn language_from_country(country_code: &str) -> Option<String> {
    let name = match country_code {
        "JP" => "Japanese",
        "KR" => "Korean",
        "CN" | "TW" | "HK" => "Chinese",
        "US" | "GB" | "AU" | "CA" | "NZ" | "IE" => "English",
        "FR" | "BE" | "CH" => "French",
        "ES" | "MX" | "AR" | "CL" | "CO" | "PE" => "Spanish",
        "DE" | "AT" => "German",
        "IT" => "Italian",
        "PT" | "BR" => "Portuguese",
        "RU" => "Russian",
        _ => return None,
    };
    Some(name.to_string())
}

fn country_name_from_code(code: &str) -> Option<String> {
    let name = match code {
        "JP" => "Japan",
        "KR" => "South Korea",
        "CN" => "China",
        "TW" => "Taiwan",
        "HK" => "Hong Kong",
        "US" => "United States",
        "GB" => "United Kingdom",
        "FR" => "France",
        "ES" => "Spain",
        "DE" => "Germany",
        "IT" => "Italy",
        "BR" => "Brazil",
        "CA" => "Canada",
        "AU" => "Australia",
        _ => return None,
    };
    Some(name.to_string())
}

fn dedupe_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

fn clean_anilist_synopsis(input: &str) -> String {
    let without_tags = strip_html_with_breaks(input);
    let decoded = decode_basic_html_entities(&without_tags);
    let without_sources = remove_source_blocks(&decoded);
    normalize_newlines(&without_sources)
}

fn strip_html_with_breaks(input: &str) -> String {
    // Strips tags while converting <br> (and <br/>, <br />) into newlines.
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '<' {
            out.push(ch);
            continue;
        }
        let mut tag = String::new();
        for c in chars.by_ref() {
            if c == '>' {
                break;
            }
            tag.push(c);
        }
        let tag = tag.trim().trim_start_matches('/').trim();
        if tag.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("br")) {
            out.push('\n');
        }
    }
    out
}

fn decode_basic_html_entities(input: &str) -> String {
    // Minimal entity decoding for AniList descriptions.
    // Supports common named entities and numeric (decimal/hex) entities.
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            out.push(ch);
            continue;
        }
        let mut entity = String::new();
        while let Some(&c) = chars.peek() {
            chars.next();
            if c == ';' {
                break;
            }
            if entity.len() > 32 {
                entity.clear();
                break;
            }
            entity.push(c);
        }
        if entity.is_empty() {
            out.push('&');
            continue;
        }
        let decoded = match entity.as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(c) = decoded {
            out.push(c);
        } else {
            out.push('&');
            out.push_str(&entity);
            out.push(';');
        }
    }
    out
}

fn remove_source_blocks(input: &str) -> String {
    // Remove "(Source: ...)" blocks (case-insensitive), common in AniList blurbs.
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut idx = 0;
    while let Some(pos) = lower[idx..].find("(source:") {
        let start = idx + pos;
        out.push_str(&input[idx..start]);
        let rest = &lower[start..];
        if let Some(end_rel) = rest.find(')') {
            idx = start + end_rel + 1;
        } else {
            idx = input.len();
            break;
        }
    }
    out.push_str(&input[idx..]);
    out
}

fn normalize_newlines(input: &str) -> String {
    let input = input.replace("\r\n", "\n");
    let mut out = String::with_capacity(input.len());
    let mut nl_run = 0usize;

    for ch in input.chars() {
        if ch == '\n' {
            nl_run += 1;
            if nl_run <= 2 {
                out.push('\n');
            }
            continue;
        }
        nl_run = 0;
        out.push(ch);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_full_fuzzy_date_only_when_complete() {
        let d = FuzzyDate {
            year: Some(2024),
            month: Some(1),
            day: Some(2),
        };
        assert_eq!(fuzzy_date_to_string(&d).as_deref(), Some("2024-01-02"));
        let d2 = FuzzyDate {
            year: Some(2024),
            month: Some(1),
            day: None,
        };
        assert_eq!(fuzzy_date_to_string(&d2), None);
    }

    #[test]
    fn title_selection_prefers_english_and_uses_romaji_as_original() {
        let t = MediaTitle {
            english: Some("English Season 2".to_string()),
            romaji: Some("Romaji Season 2".to_string()),
        };
        let (name, eng, original) = choose_titles(&t);
        assert_eq!(name, "English");
        assert_eq!(eng, None);
        assert_eq!(original.as_deref(), Some("Romaji"));
    }

    #[test]
    fn strips_trailing_season_suffix_only_at_end() {
        assert_eq!(
            strip_trailing_season_suffix("One-Punch Man Season 2"),
            "One-Punch Man"
        );
        assert_eq!(
            strip_trailing_season_suffix("One-Punch Man: Season 2"),
            "One-Punch Man"
        );
        assert_eq!(
            strip_trailing_season_suffix("One-Punch Man - Season 2"),
            "One-Punch Man"
        );
        assert_eq!(
            strip_trailing_season_suffix("Solo Leveling Season 2 - Arise"),
            "Solo Leveling Season 2 - Arise"
        );
        assert_eq!(strip_trailing_season_suffix("Season 2"), "Season 2");
    }

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

    #[test]
    fn director_role_matching_is_reasonable() {
        assert!(is_director_role("Director"));
        assert!(is_director_role("Series Director"));
        assert!(is_director_role("Chief Director"));
        assert!(!is_director_role("Assistant Director"));
    }

    #[test]
    fn content_rating_derived_from_is_adult() {
        assert_eq!(content_rating_from_is_adult(false), "All Audiences");
        assert_eq!(content_rating_from_is_adult(true), "Adult");
    }

    #[test]
    fn parses_anilist_id_only_for_digits() {
        assert_eq!(parse_anilist_id("176496"), Some(176496));
        assert_eq!(parse_anilist_id(" 176496 "), Some(176496));
        assert_eq!(parse_anilist_id("tt123"), None);
        assert_eq!(parse_anilist_id("abc"), None);
        assert_eq!(parse_anilist_id(""), None);
    }

    #[test]
    fn dedupes_preserving_first_occurrence() {
        let input = vec![
            "A".to_string(),
            "B".to_string(),
            "A".to_string(),
            "B".to_string(),
        ];
        assert_eq!(
            dedupe_preserve_order(input),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn cleans_anilist_synopsis_html_and_source() {
        let raw = "The third season of <i>One Punch Man</i>.<br><br>\n(Source: EMOTION Label YouTube Channel Description)<br><br>\n<i>Note: Excludes recap.</i>";
        let cleaned = clean_anilist_synopsis(raw);
        assert!(!cleaned.contains("<i>"));
        assert!(!cleaned.contains("<br"));
        assert!(!cleaned.to_ascii_lowercase().contains("source:"));
        assert!(cleaned.contains("The third season of One Punch Man."));
        assert!(cleaned.contains("Note: Excludes recap."));
    }
}

#[async_trait]
impl AniListApi for AniListClient {
    async fn resolve_anime_id(&self, query: &str, season: Option<i32>) -> Result<i32> {
        self.resolve_id_with_season(AniListMediaType::Anime, query, season)
            .await
    }

    async fn fetch_anime(&self, id: i32) -> Result<AniListMapped> {
        self.fetch_mapped(AniListMediaType::Anime, id).await
    }
}
