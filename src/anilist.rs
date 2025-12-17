use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
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
            synopsis: media.description,
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

    let eng_name = None;

    let original_title = romaji.map(|r| r.to_string());

    (actual, eng_name, original_title)
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
            english: Some("English".to_string()),
            romaji: Some("Romaji".to_string()),
        };
        let (name, eng, original) = choose_titles(&t);
        assert_eq!(name, "English");
        assert_eq!(eng, None);
        assert_eq!(original.as_deref(), Some("Romaji"));
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
}
