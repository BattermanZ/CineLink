use anyhow::{Result, Context, anyhow};
use log::{debug, error};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

const TMDB_BASE_URL: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Serialize, Deserialize)]
pub struct TvShowDetails {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub overview: String,
    pub videos: Option<Videos>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TvSeasonDetails {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub overview: String,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    pub season_number: i32,
    pub episodes: Vec<Episode>,
    #[serde(default)]
    pub credits: Option<Credits>,
    #[serde(default)]
    pub videos: Option<Videos>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Episode {
    pub name: String,
    pub overview: String,
    pub episode_number: i32,
    pub air_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastMember>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    pub character: String,
    pub profile_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Videos {
    #[serde(default)]
    pub results: Vec<Video>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Video {
    pub key: String,
    pub site: String,
    #[serde(rename = "type")]
    pub video_type: String,
}

pub async fn get_tv_show_details(
    client: &Client,
    tv_show_id: i32,
) -> Result<TvShowDetails> {
    let api_key = env::var("TMDB_API_KEY").context("TMDB_API_KEY must be set")?;
    
    let url = format!(
        "{}/tv/{}?api_key={}&append_to_response=videos&language=en-US",
        TMDB_BASE_URL, tv_show_id, api_key
    );

    debug!("Fetching TV show details from TMDB: show_id={}", tv_show_id);
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to fetch TV show details")?;

    let status = response.status();
    let response_text = response.text().await?;

    if !status.is_success() {
        error!("TMDB API error: {} for show_id={}\nResponse: {}", 
            status, tv_show_id, response_text);
        return Err(anyhow!("Failed to fetch TV show details: {}", status));
    }

    debug!("TMDB Show Response: {}", response_text);

    let show_details: TvShowDetails = serde_json::from_str(&response_text)
        .with_context(|| format!("Failed to parse TV show details from response: {}", response_text))?;

    Ok(show_details)
}

pub async fn get_tv_season_details(
    client: &Client,
    tv_show_id: i32,
    season_number: i32,
) -> Result<TvSeasonDetails> {
    // First get the show details for overview and videos
    let show_details = get_tv_show_details(client, tv_show_id).await?;
    
    let api_key = env::var("TMDB_API_KEY").context("TMDB_API_KEY must be set")?;
    
    // Then get season details
    let url = format!(
        "{}/tv/{}/season/{}?api_key={}&append_to_response=credits&language=en-US",
        TMDB_BASE_URL, tv_show_id, season_number, api_key
    );

    debug!("Fetching TV season details from TMDB: show_id={}, season={}", tv_show_id, season_number);
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to fetch TV season details")?;

    let status = response.status();
    let response_text = response.text().await?;

    if !status.is_success() {
        error!("TMDB API error: {} for show_id={}, season={}\nResponse: {}", 
            status, tv_show_id, season_number, response_text);
        return Err(anyhow!("Failed to fetch TV season details: {}", status));
    }

    debug!("TMDB Season Response: {}", response_text);

    let mut season_details: TvSeasonDetails = serde_json::from_str(&response_text)
        .with_context(|| format!("Failed to parse TV season details from response: {}", response_text))?;

    // Use show overview if season overview is empty
    if season_details.overview.is_empty() {
        season_details.overview = show_details.overview;
    }

    // Use show videos if season has none
    if season_details.videos.is_none() || 
       season_details.videos.as_ref().map(|v| v.results.is_empty()).unwrap_or(true) {
        season_details.videos = show_details.videos;
    }

    Ok(season_details)
}

pub async fn search_tv_show(client: &Client, query: &str) -> Result<i32> {
    let api_key = env::var("TMDB_API_KEY").context("TMDB_API_KEY must be set")?;
    
    let url = format!(
        "{}/search/tv?api_key={}&query={}&language=en-US",
        TMDB_BASE_URL, api_key, urlencoding::encode(query)
    );

    debug!("Searching for TV show on TMDB: query={}", query);
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to search TV show")?;

    let status = response.status();
    let response_text = response.text().await?;

    if !status.is_success() {
        error!("TMDB API error: {} for query={}\nResponse: {}", 
            status, query, response_text);
        return Err(anyhow!("Failed to search TV show: {}", status));
    }

    debug!("TMDB Search Response for {}: {}", query, response_text);

    #[derive(Deserialize)]
    struct SearchResponse {
        results: Vec<SearchResult>,
    }

    #[derive(Deserialize)]
    struct SearchResult {
        id: i32,
        name: String,
    }

    let search_response: SearchResponse = serde_json::from_str(&response_text)
        .with_context(|| format!("Failed to parse search response: {}", response_text))?;

    let first_result = search_response.results.first()
        .ok_or_else(|| anyhow!("No TV show found for query: {}", query))?;
    
    debug!("Found TV show: id={}, name={}", first_result.id, first_result.name);
    Ok(first_result.id)
} 