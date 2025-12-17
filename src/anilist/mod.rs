use anyhow::Result;
use async_trait::async_trait;

mod client;
mod map;
mod resolve;
mod text;

pub use client::{AniListClient, AniListMediaType};
pub(crate) use map::strip_trailing_season_suffix;

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
