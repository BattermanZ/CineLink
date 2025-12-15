use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::env;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const POSTER_BASE: &str = "https://image.tmdb.org/t/p/original";

#[derive(Debug, Clone)]
pub struct TmdbClient {
    client: Client,
    api_key: String,
}

#[async_trait]
pub trait TmdbApi: Send + Sync {
    async fn search_movie(&self, query: &str) -> Result<i32>;
    async fn search_tv(&self, query: &str) -> Result<i32>;
    async fn resolve_movie_id(&self, query: &str) -> Result<i32>;
    async fn resolve_tv_id(&self, query: &str) -> Result<i32>;
    async fn fetch_movie(&self, id: i32) -> Result<MediaData>;
    async fn fetch_tv_season(&self, id: i32, season: i32) -> Result<MediaData>;
}

#[derive(Debug, Clone)]
pub struct MediaData {
    pub id: i32,
    pub name: String,
    pub eng_name: String,
    pub synopsis: Option<String>,
    pub genres: Vec<String>,
    pub cast: Vec<String>,
    pub director: Vec<String>,
    pub content_rating: Option<String>,
    pub country_of_origin: Vec<String>,
    pub language: Option<String>,
    pub release_date: Option<String>,
    pub year: Option<String>,
    pub runtime_minutes: Option<f32>,
    pub episodes: Option<usize>,
    pub trailer: Option<String>,
    pub poster: Option<String>,
    #[allow(dead_code)]
    pub backdrop: Option<String>,
    pub imdb_page: Option<String>,
}

impl TmdbClient {
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("TMDB_API_KEY").context("TMDB_API_KEY not set")?;
        Ok(Self {
            client: Client::new(),
            api_key,
        })
    }
}

#[async_trait]
impl TmdbApi for TmdbClient {
    async fn search_movie(&self, query: &str) -> Result<i32> {
        #[derive(Deserialize)]
        struct SearchResult {
            id: i32,
        }
        #[derive(Deserialize)]
        struct SearchResponse {
            results: Vec<SearchResult>,
        }

        let url = format!(
            "{TMDB_BASE}/search/movie?api_key={}&query={}&language=en-US",
            self.api_key,
            urlencoding::encode(query)
        );
        let data: SearchResponse = self.get_json(&url).await?;
        data.results
            .first()
            .map(|r| r.id)
            .ok_or_else(|| anyhow!("No TMDB movie found for '{}'", query))
    }

    async fn resolve_movie_id(&self, query: &str) -> Result<i32> {
        if let Some(id) = parse_tmdb_id(query) {
            return Ok(id);
        }
        if let Some(imdb) = parse_imdb_id(query) {
            if let Some(id) = self.find_imdb(&imdb, "movie").await? {
                return Ok(id);
            }
        }
        self.search_movie(query).await
    }

    async fn search_tv(&self, query: &str) -> Result<i32> {
        #[derive(Deserialize)]
        struct SearchResult {
            id: i32,
        }
        #[derive(Deserialize)]
        struct SearchResponse {
            results: Vec<SearchResult>,
        }

        let url = format!(
            "{TMDB_BASE}/search/tv?api_key={}&query={}&language=en-US",
            self.api_key,
            urlencoding::encode(query)
        );
        let data: SearchResponse = self.get_json(&url).await?;
        data.results
            .first()
            .map(|r| r.id)
            .ok_or_else(|| anyhow!("No TMDB TV show found for '{}'", query))
    }

    async fn resolve_tv_id(&self, query: &str) -> Result<i32> {
        if let Some(id) = parse_tmdb_id(query) {
            return Ok(id);
        }
        if let Some(imdb) = parse_imdb_id(query) {
            if let Some(id) = self.find_imdb(&imdb, "tv").await? {
                return Ok(id);
            }
        }
        self.search_tv(query).await
    }

    async fn fetch_movie(&self, id: i32) -> Result<MediaData> {
        let detail: MovieDetail = self
            .get_json(&format!(
                "{TMDB_BASE}/movie/{id}?language=en-US&api_key={}",
                self.api_key
            ))
            .await?;
        let credits: Credits = self
            .get_json(&format!(
                "{TMDB_BASE}/movie/{id}/credits?api_key={}",
                self.api_key
            ))
            .await?;
        let release_dates: ReleaseDates = self
            .get_json(&format!(
                "{TMDB_BASE}/movie/{id}/release_dates?api_key={}",
                self.api_key
            ))
            .await?;
        let videos: Videos = self
            .get_json(&format!(
                "{TMDB_BASE}/movie/{id}/videos?api_key={}",
                self.api_key
            ))
            .await?;
        let external_ids: ExternalIds = self
            .get_json(&format!(
                "{TMDB_BASE}/movie/{id}/external_ids?api_key={}",
                self.api_key
            ))
            .await?;

        let content_rating = us_cert_from_release_dates(&release_dates);
        let director = credits
            .crew
            .unwrap_or_default()
            .into_iter()
            .filter(|c| matches!(c.job.as_deref(), Some("Director")))
            .map(|c| c.name)
            .collect::<Vec<_>>();
        let cast = top_names(&credits.cast, 10);
        let trailer = select_trailer(&videos);
        let poster = detail
            .poster_path
            .as_ref()
            .map(|p| format!("{POSTER_BASE}{p}"));
        let backdrop = detail
            .backdrop_path
            .as_ref()
            .map(|p| format!("{POSTER_BASE}{p}"));
        let country = origin_country(
            detail.origin_country.as_ref(),
            detail.production_countries.as_ref(),
        );
        let genres = names(detail.genres.as_ref());
        let release_date = detail.release_date.clone();
        let year = release_date.as_deref().and_then(extract_year);
        let imdb_page = external_ids
            .imdb_id
            .as_ref()
            .map(|id| format!("https://www.imdb.com/title/{id}"));
        let language = language_name(&detail.original_language);

        Ok(MediaData {
            id: detail.id,
            name: detail.title.clone(),
            eng_name: detail.title.clone(),
            synopsis: Some(detail.overview),
            genres,
            cast,
            director,
            content_rating,
            country_of_origin: country,
            language,
            release_date,
            year,
            runtime_minutes: detail.runtime,
            episodes: None,
            trailer,
            poster,
            backdrop,
            imdb_page,
        })
    }

    async fn fetch_tv_season(&self, id: i32, season: i32) -> Result<MediaData> {
        let show: ShowDetail = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}?language=en-US&api_key={}",
                self.api_key
            ))
            .await?;
        let season_detail: SeasonDetail = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}/season/{season}?language=en-US&api_key={}",
                self.api_key
            ))
            .await?;
        let credits: Credits = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}/season/{season}/credits?api_key={}",
                self.api_key
            ))
            .await?;
        let ratings: ContentRatings = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}/content_ratings?api_key={}",
                self.api_key
            ))
            .await?;
        let videos: Videos = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}/season/{season}/videos?api_key={}",
                self.api_key
            ))
            .await?;
        let show_videos: Videos = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}/videos?api_key={}",
                self.api_key
            ))
            .await?;
        let external_ids: ExternalIds = self
            .get_json(&format!(
                "{TMDB_BASE}/tv/{id}/external_ids?api_key={}",
                self.api_key
            ))
            .await?;

        let content_rating = us_rating(&ratings);
        let cast = top_names(&credits.cast, 10);
        let trailer = select_trailer(&videos).or_else(|| select_trailer(&show_videos));
        let poster = season_detail
            .poster_path
            .as_ref()
            .or(show.poster_path.as_ref())
            .map(|p| format!("{POSTER_BASE}{p}"));
        let backdrop = show
            .backdrop_path
            .as_ref()
            .map(|p| format!("{POSTER_BASE}{p}"));
        let country = origin_country(Some(&show.origin_country), None);
        let genres = names(show.genres.as_ref());
        let air_date = season_detail.air_date.clone();
        let year = air_date.as_deref().and_then(extract_year);
        let imdb_page = external_ids
            .imdb_id
            .as_ref()
            .map(|id| format!("https://www.imdb.com/title/{id}"));
        let created_by = show
            .created_by
            .as_ref()
            .map(|c| c.iter().map(|c| c.name.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        let episodes_count = season_detail.episodes.len();
        let runtime = average_episode_runtime(&season_detail, &show);
        let language = language_name(&show.original_language);

        Ok(MediaData {
            id: show.id,
            name: show.name.clone(),
            eng_name: show.name.clone(),
            synopsis: Some(if season_detail.overview.is_empty() {
                show.overview.clone()
            } else {
                season_detail.overview.clone()
            }),
            genres,
            cast,
            director: created_by,
            content_rating,
            country_of_origin: country,
            language,
            release_date: air_date,
            year,
            runtime_minutes: runtime,
            episodes: Some(episodes_count),
            trailer,
            poster,
            backdrop,
            imdb_page,
        })
    }
}

impl TmdbClient {
    async fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let res = self
            .client
            .get(url)
            .send()
            .await
            .context("request failed")?;
        let status = res.status();
        let text = res.text().await.context("reading body failed")?;
        if !status.is_success() {
            return Err(anyhow!("{} -> {}", url, text));
        }
        let parsed: T = serde_json::from_str(&text).context("JSON parse failed")?;
        Ok(parsed)
    }

    async fn find_imdb(&self, imdb_id: &str, media: &str) -> Result<Option<i32>> {
        #[derive(Deserialize)]
        struct FindResponse {
            movie_results: Option<Vec<FindResult>>,
            tv_results: Option<Vec<FindResult>>,
        }
        #[derive(Deserialize)]
        struct FindResult {
            id: i32,
        }

        let url = format!(
            "{TMDB_BASE}/find/{imdb_id}?external_source=imdb_id&language=en-US&api_key={}",
            self.api_key
        );
        let data: FindResponse = self.get_json(&url).await?;
        let id = match media {
            "movie" => data
                .movie_results
                .and_then(|mut v| v.pop())
                .map(|r| r.id),
            "tv" => data.tv_results.and_then(|mut v| v.pop()).map(|r| r.id),
            _ => None,
        };
        Ok(id)
    }
}

#[derive(Debug, Deserialize)]
struct Genre {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ProductionCountry {
    iso_3166_1: String,
}

#[derive(Debug, Deserialize)]
struct MovieDetail {
    id: i32,
    title: String,
    overview: String,
    release_date: Option<String>,
    runtime: Option<f32>,
    original_language: String,
    origin_country: Option<Vec<String>>,
    production_countries: Option<Vec<ProductionCountry>>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genres: Option<Vec<Genre>>,
}

#[derive(Debug, Deserialize)]
struct ShowDetail {
    id: i32,
    name: String,
    overview: String,
    original_language: String,
    origin_country: Vec<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genres: Option<Vec<Genre>>,
    episode_run_time: Option<Vec<i32>>,
    created_by: Option<Vec<Creator>>,
}

#[derive(Debug, Deserialize)]
struct Creator {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SeasonDetail {
    overview: String,
    air_date: Option<String>,
    poster_path: Option<String>,
    episodes: Vec<Episode>,
}

#[derive(Debug, Deserialize)]
struct Episode {
    runtime: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct Credits {
    cast: Vec<CastMember>,
    crew: Option<Vec<CrewMember>>,
}

#[derive(Debug, Deserialize)]
struct CastMember {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CrewMember {
    job: Option<String>,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ContentRatings {
    results: Vec<RatingEntry>,
}

#[derive(Debug, Deserialize)]
struct RatingEntry {
    iso_3166_1: String,
    rating: Option<String>,
    certification: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseDates {
    results: Vec<ReleaseEntry>,
}

#[derive(Debug, Deserialize)]
struct ReleaseEntry {
    iso_3166_1: String,
    release_dates: Vec<ReleaseCert>,
}

#[derive(Debug, Deserialize)]
struct ReleaseCert {
    certification: String,
}

#[derive(Debug, Deserialize)]
struct ExternalIds {
    imdb_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Videos {
    results: Vec<Video>,
}

#[derive(Debug, Deserialize)]
struct Video {
    site: String,
    #[serde(rename = "type")]
    video_type: String,
    key: String,
}

pub fn parse_season_number(input: &str) -> Option<i32> {
    if input.eq_ignore_ascii_case("Mini-series") {
        return Some(1);
    }
    if let Some(rest) = input.strip_prefix("Season ") {
        return rest.trim().parse::<i32>().ok();
    }
    input.trim().parse().ok()
}

pub fn parse_tmdb_id(input: &str) -> Option<i32> {
    if input.chars().all(|c| c.is_ascii_digit()) {
        return input.parse().ok();
    }
    None
}

pub fn parse_imdb_id(input: &str) -> Option<String> {
    let lower = input.trim().to_lowercase();
    if lower.starts_with("tt") && lower.len() > 2 && lower[2..].chars().all(|c| c.is_ascii_digit())
    {
        return Some(lower);
    }
    None
}

fn us_cert_from_release_dates(data: &ReleaseDates) -> Option<String> {
    data.results
        .iter()
        .find(|r| r.iso_3166_1 == "US")
        .and_then(|r| {
            r.release_dates
                .iter()
                .find(|rd| !rd.certification.is_empty())
        })
        .map(|rd| rd.certification.clone())
}

fn us_rating(data: &ContentRatings) -> Option<String> {
    data.results
        .iter()
        .find(|r| r.iso_3166_1 == "US")
        .and_then(|r| r.rating.clone().or(r.certification.clone()))
}

fn top_names(list: &[CastMember], max: usize) -> Vec<String> {
    list.iter().take(max).map(|c| c.name.clone()).collect()
}

fn names(genres: Option<&Vec<Genre>>) -> Vec<String> {
    genres
        .map(|g| g.iter().map(|x| x.name.clone()).collect())
        .unwrap_or_default()
}

fn origin_country(
    origin: Option<&Vec<String>>,
    production: Option<&Vec<ProductionCountry>>,
) -> Vec<String> {
    if let Some(o) = origin {
        if !o.is_empty() {
            return o.clone();
        }
    }
    production
        .map(|p| p.iter().map(|c| c.iso_3166_1.clone()).collect())
        .unwrap_or_default()
}

fn extract_year(date: &str) -> Option<String> {
    date.split('-').next().map(|s| s.to_string())
}

fn select_trailer(videos: &Videos) -> Option<String> {
    videos
        .results
        .iter()
        .find(|v| v.site.eq_ignore_ascii_case("YouTube") && v.video_type == "Trailer")
        .or_else(|| {
            videos
                .results
                .iter()
                .find(|v| v.site.eq_ignore_ascii_case("YouTube") && v.video_type == "Teaser")
        })
        .map(|v| format!("https://www.youtube.com/watch?v={}", v.key))
}

fn average_episode_runtime(season: &SeasonDetail, show: &ShowDetail) -> Option<f32> {
    let runtimes: Vec<i32> = season.episodes.iter().filter_map(|e| e.runtime).collect();
    if !runtimes.is_empty() {
        let sum: i32 = runtimes.iter().sum();
        return Some(sum as f32 / runtimes.len() as f32);
    }
    show.episode_run_time
        .as_ref()
        .and_then(|r| r.first().copied())
        .map(|r| r as f32)
}

fn language_name(code: &str) -> Option<String> {
    let name = match code {
        "en" => "English",
        "fr" => "French",
        "es" => "Spanish",
        "de" => "German",
        "it" => "Italian",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "zh" => "Chinese",
        "ar" => "Arabic",
        "hi" => "Hindi",
        "sv" => "Swedish",
        "da" => "Danish",
        "no" => "Norwegian",
        "fi" => "Finnish",
        "nl" => "Dutch",
        "pl" => "Polish",
        "tr" => "Turkish",
        "cs" => "Czech",
        "el" => "Greek",
        "he" => "Hebrew",
        "id" => "Indonesian",
        "ms" => "Malay",
        "th" => "Thai",
        "vi" => "Vietnamese",
        "ro" => "Romanian",
        "hu" => "Hungarian",
        "uk" => "Ukrainian",
        "fa" => "Persian",
        _ => return Some(code.to_string()),
    };
    Some(name.to_string())
}
