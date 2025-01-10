use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Movie {
    pub title: String,
    pub rating: f32,
    pub rating_key: String,
    pub library_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TvShow {
    pub title: String,
    pub rating: f32,
    pub rating_key: String,
    pub library_id: String,
    pub season_number: i32,
    pub tmdb_id: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TvSeason {
    pub show_title: String,
    pub season_number: i32,
    pub overview: String,
    pub poster_url: Option<String>,
    pub air_date: Option<String>,
    pub cast: Vec<String>,
    pub trailer_url: Option<String>,
    pub rating: Option<f32>,
    pub rating_key: String,
    pub library_id: String,
}

