//! Fetch TMDB metadata for a movie or TV season and print mapped properties.
//! Usage:
//!   cargo run --bin tmdb_props -- movie <tmdb_id>
//!   cargo run --bin tmdb_props -- tv <tmdb_id> <season_number>
//! Requires TMDB_API_KEY in the environment (.env supported).

use anyhow::{Context, Result};
use dotenvy::dotenv;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::str::FromStr;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const POSTER_BASE: &str = "https://image.tmdb.org/t/p/original";

#[derive(Debug, Clone, Copy, PartialEq)]
enum MediaKind {
    Movie,
    Tv,
}

impl FromStr for MediaKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "movie" => Ok(MediaKind::Movie),
            "tv" => Ok(MediaKind::Tv),
            _ => Err(anyhow::anyhow!("media kind must be 'movie' or 'tv'")),
        }
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
    #[allow(dead_code)]
    first_air_date: Option<String>,
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
    #[allow(dead_code)]
    name: String,
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
    // release_dates endpoint uses certification; we treat rating as generic.
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: cargo run --bin tmdb_props -- movie <tmdb_id>");
        eprintln!("       cargo run --bin tmdb_props -- tv <tmdb_id> <season_number>");
        std::process::exit(1);
    }

    let kind = MediaKind::from_str(&args[1])?;
    let tmdb_id: i32 = args[2].parse().context("tmdb_id must be an integer")?;
    let season_number: Option<i32> = if kind == MediaKind::Tv {
        Some(
            args.get(3)
                .ok_or_else(|| anyhow::anyhow!("missing season number for tv"))?
                .parse()
                .context("season number must be an integer")?,
        )
    } else {
        None
    };

    let api_key = env::var("TMDB_API_KEY").context("TMDB_API_KEY not set")?;
    let client = Client::new();

    match kind {
        MediaKind::Movie => fetch_movie(&client, tmdb_id, &api_key).await?,
        MediaKind::Tv => {
            let season = season_number.expect("season required for tv");
            fetch_tv_season(&client, tmdb_id, season, &api_key).await?
        }
    }

    Ok(())
}

async fn fetch_movie(client: &Client, id: i32, api_key: &str) -> Result<()> {
    let detail: MovieDetail = get_json(
        client,
        &format!("{TMDB_BASE}/movie/{id}?language=en-US&api_key={api_key}"),
    )
    .await?;
    let credits: Credits = get_json(
        client,
        &format!("{TMDB_BASE}/movie/{id}/credits?api_key={api_key}"),
    )
    .await?;
    let release_dates: ReleaseDates = get_json(
        client,
        &format!("{TMDB_BASE}/movie/{id}/release_dates?api_key={api_key}"),
    )
    .await?;
    let videos: Videos = get_json(
        client,
        &format!("{TMDB_BASE}/movie/{id}/videos?api_key={api_key}"),
    )
    .await?;
    let external_ids: ExternalIds = get_json(
        client,
        &format!("{TMDB_BASE}/movie/{id}/external_ids?api_key={api_key}"),
    )
    .await?;

    let content_rating = us_cert_from_release_dates(&release_dates);
    let director = credits
        .crew
        .unwrap_or_default()
        .into_iter()
        .find(|c| matches!(c.job.as_deref(), Some("Director")))
        .map(|c| c.name);
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

    let output = json!({
        "id": detail.id,
        "name": detail.title,
        "eng_name": detail.title,
        "synopsis": detail.overview,
        "genre": genres,
        "cast": cast,
        "director": director,
        "content_rating": content_rating,
        "country_of_origin": country,
        "language": detail.original_language,
        "release_date": release_date,
        "year": year,
        "runtime_minutes": detail.runtime,
        "episodes": Value::Null,
        "trailer": trailer,
        "img": poster,
        "backdrop": backdrop,
        "imdb_page": imdb_page,
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

async fn fetch_tv_season(client: &Client, id: i32, season: i32, api_key: &str) -> Result<()> {
    let show: ShowDetail = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}?language=en-US&api_key={api_key}"),
    )
    .await?;
    let season_detail: SeasonDetail = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}/season/{season}?language=en-US&api_key={api_key}"),
    )
    .await?;
    let credits: Credits = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}/season/{season}/credits?api_key={api_key}"),
    )
    .await?;
    let ratings: ContentRatings = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}/content_ratings?api_key={api_key}"),
    )
    .await?;
    let videos: Videos = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}/season/{season}/videos?api_key={api_key}"),
    )
    .await?;
    let show_videos: Videos = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}/videos?api_key={api_key}"),
    )
    .await?;
    let external_ids: ExternalIds = get_json(
        client,
        &format!("{TMDB_BASE}/tv/{id}/external_ids?api_key={api_key}"),
    )
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

    let output = json!({
        "id": show.id,
        "name": show.name,
        "eng_name": show.name,
        "synopsis": if season_detail.overview.is_empty() { show.overview.clone() } else { season_detail.overview.clone() },
        "genre": genres,
        "cast": cast,
        "director": if created_by.is_empty() { None } else { Some(created_by) },
        "content_rating": content_rating,
        "country_of_origin": country,
        "language": show.original_language,
        "release_date": air_date,
        "year": year,
        "runtime_minutes": runtime,
        "episodes": episodes_count,
        "trailer": trailer,
        "img": poster,
        "backdrop": backdrop,
        "imdb_page": imdb_page,
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
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

fn top_names(list: &[CastMember], max: usize) -> Option<Vec<String>> {
    if list.is_empty() {
        None
    } else {
        Some(list.iter().take(max).map(|c| c.name.clone()).collect())
    }
}

fn names(genres: Option<&Vec<Genre>>) -> Option<Vec<String>> {
    genres.map(|g| g.iter().map(|x| x.name.clone()).collect())
}

fn origin_country(
    origin: Option<&Vec<String>>,
    production: Option<&Vec<ProductionCountry>>,
) -> Option<Vec<String>> {
    if let Some(o) = origin {
        if !o.is_empty() {
            return Some(o.clone());
        }
    }
    production.map(|p| p.iter().map(|c| c.iso_3166_1.clone()).collect())
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

async fn get_json<T: for<'de> Deserialize<'de>>(client: &Client, url: &str) -> Result<T> {
    let res = client.get(url).send().await.context("request failed")?;
    let status = res.status();
    let text = res.text().await.context("reading body failed")?;
    if !status.is_success() {
        return Err(anyhow::anyhow!("{} -> {}", url, text));
    }
    let parsed: T = serde_json::from_str(&text).context("JSON parse failed")?;
    Ok(parsed)
}
