use std::fs;
use log::{info, error, debug, LevelFilter};
use env_logger::Builder;
use std::io::Write;
use chrono::{Local, Datelike};
use dotenvy::dotenv;
use std::env;
use anyhow::{Result, Context, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use quick_xml::reader::Reader;
use quick_xml::events::Event;
use futures::future::join_all;
use tokio::task;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Movie {
    title: String,
    rating: f32,
    rating_key: String,
    library_id: String,
}


async fn get_all_movies(client: &Client, plex_url: &str, plex_token: &str) -> Result<(Vec<Movie>, Vec<Movie>)> {
    let url = format!("{}/library/sections/all?X-Plex-Token={}", plex_url, plex_token);
    let response = client.get(&url).send().await?.text().await?;

    let mut reader = Reader::from_str(&response);
    let mut libraries = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if e.name().as_ref() == b"Directory" {
                    let mut is_movie_library = false;
                    let mut library_id = String::new();

                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"type" => {
                                if attr.value.as_ref() == b"movie" {
                                    is_movie_library = true;
                                }
                            },
                            b"key" => {
                                library_id = attr.unescape_value()?.into_owned();
                            },
                            _ => {}
                        }
                    }

                    if is_movie_library {
                        libraries.push(library_id);
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error parsing XML: {:?}", e)),
            _ => {}
        }
        buf.clear();
    }

    let mut all_movies = Vec::new();

    for library_id in libraries {
        let url = format!("{}/library/sections/{}/all?X-Plex-Token={}", plex_url, library_id, plex_token);
        let response = client.get(&url).send().await?.text().await?;

        let mut reader = Reader::from_str(&response);
        let mut buf = Vec::new();
        let mut current_movie: Option<Movie> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    if e.name().as_ref() == b"Video" {
                        let mut title = String::new();
                        let mut rating = 0.0;
                        let mut rating_key = String::new();

                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"title" => {
                                    title = attr.unescape_value()?.into_owned();
                                },
                                b"userRating" => {
                                    if let Ok(user_rating) = attr.unescape_value()?.parse::<f32>() {
                                        rating = user_rating;
                                    }
                                },
                                b"ratingKey" => {
                                    rating_key = attr.unescape_value()?.into_owned();
                                },
                                _ => {}
                            }
                        }

                        if !title.is_empty() && !rating_key.is_empty() {
                            current_movie = Some(Movie { title, rating, rating_key, library_id: library_id.clone() });
                        }
                    }
                },
                Ok(Event::End(ref e)) => {
                    if e.name().as_ref() == b"Video" {
                        if let Some(movie) = current_movie.take() {
                            all_movies.push(movie);
                        }
                    }
                },
                Ok(Event::Eof) => break,
                Err(e) => return Err(anyhow!("Error parsing XML: {:?}", e)),
                _ => {}
            }
            buf.clear();
        }
    }

    let total_movies = all_movies.len();
    let rated_movies: Vec<Movie> = all_movies.iter()
        .filter(|movie| movie.rating > 0.0)
        .cloned()
        .collect();

    info!("Retrieved {} movies from Plex, {} with ratings.", total_movies, rated_movies.len());
    for movie in &rated_movies {
        debug!("Rated movie from Plex: {} | User Rating: {} | Rating Key: {} | Library ID: {}", movie.title, movie.rating, movie.rating_key, movie.library_id);
    }

    Ok((all_movies, rated_movies))
}

fn numeric_to_emoji_rating(numeric_rating: f32) -> &'static str {
    match numeric_rating as i32 {
        1 => "🌗",
        2 => "🌕",
        3 => "🌕🌗",
        4 => "🌕🌕",
        5 => "🌕🌕🌗",
        6 => "🌕🌕🌕",
        7 => "🌕🌕🌕🌗",
        8 => "🌕🌕🌕🌕",
        9 => "🌕🌕🌕🌕🌗",
        10 => "🌕🌕🌕🌕🌕",
        _ => "🌕",
    }
}

async fn get_notion_movie_info(client: &Client, headers: &reqwest::header::HeaderMap, movie_title: &str, database_id: &str) -> Result<(bool, Option<String>, bool)> {
    let query_url = format!("https://api.notion.com/v1/databases/{}/query", database_id);
    let query_payload = json!({
        "filter": {
            "or": [
                {"property": "Name", "title": {"contains": movie_title}},
                {"property": "Eng Name", "rich_text": {"contains": movie_title}}
            ]
        }
    });

    let response = client.post(&query_url)
        .headers(headers.clone())
        .json(&query_payload)
        .send()
        .await?;

    if response.status().is_success() {
        let result: serde_json::Value = response.json().await?;

        let empty_vec: Vec<serde_json::Value> = Vec::new();
        let results = result["results"].as_array().unwrap_or(&empty_vec);

        if results.is_empty() {
            Ok((false, None, false))
        } else {
            let page = &results[0];
            let page_id = page["id"].as_str().map(|s| s.to_string());
            let rating_emoji = page["properties"]["Aurel's rating"]["select"]["name"].as_str();
            let has_rating = rating_emoji.is_some();
            Ok((true, page_id, has_rating))
        }
    } else {
        Err(anyhow!("Failed to query Notion for '{}'. Status: {}", movie_title, response.status()))
    }
}

async fn add_movie(client: &Client, notion_url: &str, headers: &reqwest::header::HeaderMap, movie: &Movie, database_id: &str) -> Result<()> {
    let movie_title = &movie.title;
    let movie_rating = numeric_to_emoji_rating(movie.rating);
    let notion_movie_title = format!("{};", movie_title);
    let current_year = Local::now().year().to_string();

    let payload = json!({
        "parent": {"database_id": database_id},
        "properties": {
            "Name": {
                "title": [
                    {"text": {"content": notion_movie_title}}
                ]
            },
            "Aurel's rating": {
                "select": {"name": movie_rating}
            },
            "Years watched": {
                "multi_select": [{"name": current_year}]
            }
        }
    });

    let response = client.post(notion_url)
        .headers(headers.clone())
        .json(&payload)
        .send()
        .await?;

    if response.status().is_success() {
        info!("Movie '{}' added to Notion with rating: '{}', Year: '{}'", movie_title, movie_rating, current_year);
        Ok(())
    } else {
        Err(anyhow!("Failed to add '{}' to Notion. Status: {}", movie_title, response.status()))
    }
}

async fn update_movie_rating_in_notion(client: &Client, headers: &reqwest::header::HeaderMap, page_id: &str, movie_title: &str, rating: f32) -> Result<()> {
    let notion_page_url = format!("https://api.notion.com/v1/pages/{}", page_id);
    let movie_rating = numeric_to_emoji_rating(rating);

    let payload = json!({
        "properties": {
            "Aurel's rating": {
                "select": {
                    "name": movie_rating
                }
            }
        }
    });

    let response = client.patch(&notion_page_url)
        .headers(headers.clone())
        .json(&payload)
        .send()
        .await?;

    if response.status().is_success() {
        info!("Updated Notion rating for movie '{}' to '{}'", movie_title, movie_rating);
        Ok(())
    } else {
        Err(anyhow!("Failed to update Notion rating for movie '{}'. Status: {}", movie_title, response.status()))
    }
}

async fn add_movies_in_batch(client: &Client, notion_url: &str, headers: &reqwest::header::HeaderMap, movie_list: &[Movie], database_id: &str) -> Result<String> {
    let movie_info_futures: Vec<_> = movie_list.iter()
        .map(|movie| {
            let client = client.clone();
            let headers = headers.clone();
            let database_id = database_id.to_string();
            let title = movie.title.clone();
            let rating = movie.rating;
            task::spawn(async move {
                match get_notion_movie_info(&client, &headers, &title, &database_id).await {
                    Ok((exists, page_id, has_rating)) => (title, rating, exists, page_id, has_rating),
                    Err(_) => (title, rating, false, None, false),
                }
            })
        })
        .collect();

    let movie_info_results = join_all(movie_info_futures).await;

    let mut movies_to_add = Vec::new();

    for result in movie_info_results {
        if let Ok((title, rating, exists, page_id, has_rating)) = result {
            if !exists {
                if let Some(movie) = movie_list.iter().find(|m| m.title == title) {
                    movies_to_add.push(movie.clone());
                }
            } else {
                if !has_rating {
                    if let Some(pid) = page_id {
                        if let Err(e) = update_movie_rating_in_notion(client, headers, &pid, &title, rating).await {
                            error!("Failed to update rating for '{}': {}", title, e);
                        }
                    }
                }
            }
        }
    }

    if movies_to_add.is_empty() {
        info!("No new movies to add to Notion.");
        return Ok("No new movies to add.".to_string());
    }

    info!("Adding {} movies in batch to Notion...", movies_to_add.len());

    let add_movie_futures: Vec<_> = movies_to_add.into_iter()
        .map(|movie| {
            let client = client.clone();
            let notion_url = notion_url.to_string();
            let headers = headers.clone();
            let database_id = database_id.to_string();
            task::spawn(async move {
                add_movie(&client, &notion_url, &headers, &movie, &database_id).await
            })
        })
        .collect();

    let results = join_all(add_movie_futures).await;
    let successful_additions = results.into_iter().filter(|r| r.as_ref().map_or(false, |inner_r| inner_r.is_ok())).count();

    Ok(format!("Batch processing completed. Successfully added {} movies.", successful_additions))
}

async fn get_all_notion_movies(client: &Client, headers: &reqwest::header::HeaderMap, database_id: &str) -> Result<Vec<Movie>> {
    let query_url = format!("https://api.notion.com/v1/databases/{}/query", database_id);
    let query_payload = json!({
        "filter": {
            "property": "Aurel's rating",
            "select": {
                "is_not_empty": true
            }
        }
    });

    let response = client.post(&query_url)
        .headers(headers.clone())
        .json(&query_payload)
        .send()
        .await?;

    if response.status().is_success() {
        let result: serde_json::Value = response.json().await?;
        let movies = result["results"].as_array()
            .ok_or_else(|| anyhow!("Unexpected response format from Notion"))?
            .iter()
            .filter_map(|page| {
                let title = page["properties"]["Name"]["title"][0]["text"]["content"].as_str()?;
                let rating_emoji = page["properties"]["Aurel's rating"]["select"]["name"].as_str()?;
                let rating = match rating_emoji {
                    "🌗" => 1.0,
                    "🌕" => 2.0,
                    "🌕🌗" => 3.0,
                    "🌕🌕" => 4.0,
                    "🌕🌕🌗" => 5.0,
                    "🌕🌕🌕" => 6.0,
                    "🌕🌕🌕🌗" => 7.0,
                    "🌕🌕🌕🌕" => 8.0,
                    "🌕🌕🌕🌕🌗" => 9.0,
                    "🌕🌕🌕🌕🌕" => 10.0,
                    _ => return None,
                };
                Some(Movie {
                    title: title.trim_end_matches(';').to_string(),
                    rating,
                    rating_key: String::new(),
                    library_id: String::new(),
                })
            })
            .collect();

        Ok(movies)
    } else {
        Err(anyhow!("Failed to query Notion database. Status: {}", response.status()))
    }
}

async fn update_plex_rating(client: &Client, plex_url: &str, plex_token: &str, notion_movie: &Movie, plex_movies: &[Movie]) -> Result<()> {
    if let Some(plex_movie) = plex_movies.iter().find(|m| m.title == notion_movie.title) {
        if (plex_movie.rating - notion_movie.rating).abs() < f32::EPSILON {
            info!("Rating for '{}' is already {} in both Notion and Plex. No update needed.", notion_movie.title, notion_movie.rating);
            return Ok(());
        }

        let update_url = format!(
            "{}/library/sections/{}/all?type=1&id={}&userRating.value={}&userRating.locked=1&X-Plex-Token={}",
            plex_url, plex_movie.library_id, plex_movie.rating_key, notion_movie.rating, plex_token
        );
        debug!("Updating Plex rating using URL: {}", update_url);
        let response = client.put(&update_url).send().await?;

        let status = response.status();
        if status.is_success() {
            info!("Updated Plex rating for '{}' to {}", notion_movie.title, notion_movie.rating);
            Ok(())
        } else {
            Err(anyhow!("Failed to update Plex rating for '{}'. Status: {}", notion_movie.title, status))
        }
    } else {
        debug!("Movie '{}' not found in Plex library", notion_movie.title);
        Ok(())
    }
}

async fn sync_notion_to_plex(notion_client: &Client, plex_client: &Client, notion_headers: &reqwest::header::HeaderMap, notion_db_id: &str, plex_url: &str, plex_token: &str, plex_movies: &[Movie]) -> Result<()> {
    info!("Starting Notion to Plex sync");
    let notion_movies = get_all_notion_movies(notion_client, notion_headers, notion_db_id).await?;

    for movie in notion_movies {
        if let Err(e) = update_plex_rating(plex_client, plex_url, plex_token, &movie, plex_movies).await {
            error!("Failed to update Plex rating for '{}': {}", movie.title, e);
        }
    }

    info!("Notion to Plex sync completed");
    Ok(())
}

async fn sync_plex_to_notion(notion_client: &Client, notion_url: &str, notion_headers: &reqwest::header::HeaderMap, notion_db_id: &str, plex_movies: &[Movie]) -> Result<()> {
    info!("Starting Plex to Notion sync");

    let response = add_movies_in_batch(notion_client, notion_url, notion_headers, plex_movies, notion_db_id).await?;
    info!("Plex to Notion sync completed: {}", response);
    Ok(())
}

async fn run_bidirectional_sync(
    plex_client: &Client,
    notion_client: &Client,
    plex_url: &str,
    plex_token: &str,
    notion_url: &str,
    notion_headers: &reqwest::header::HeaderMap,
    notion_db_id: &str
) -> Result<()> {
    info!("Starting bidirectional sync");

    let (all_plex_movies, rated_plex_movies) = get_all_movies(plex_client, plex_url, plex_token).await?;

    sync_plex_to_notion(notion_client, notion_url, notion_headers, notion_db_id, &rated_plex_movies).await?;
    sync_notion_to_plex(notion_client, plex_client, notion_headers, notion_db_id, plex_url, plex_token, &all_plex_movies).await?;

    info!("Bidirectional sync completed successfully");
    Ok(())
}


fn setup_logger() -> Result<()> {
    fs::create_dir_all("logs").context("Failed to create logs directory")?;

    Builder::new()
        .filter_level(LevelFilter::Info)
        .format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] - {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .filter(Some("reqwest"), LevelFilter::Warn)
        .target(env_logger::Target::Pipe(Box::new(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("logs/cinelink.log")
                .context("Failed to open or create log file")?
        )))
        .init();
    Ok(())
}

fn check_env_var(var_name: &str) -> Result<()> {
    match env::var(var_name) {
        Ok(_) => {
            info!("Environment variable '{}' found.", var_name);
            Ok(())
        },
        Err(_) => Err(anyhow!("Environment variable '{}' not found. Please set it in your .env file.", var_name)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().context("Failed to load .env file")?;

    setup_logger()?;

    info!("CineLink starting up...");

    let env_vars = [
        "NOTION_API_KEY",
        "NOTION_DATABASE_ID",
        "PLEX_URL",
        "PLEX_TOKEN",
    ];

    for var in env_vars.iter() {
        check_env_var(var)?;
    }

    let client = Client::new();
    let plex_url = env::var("PLEX_URL")?;
    let plex_token = env::var("PLEX_TOKEN")?;
    let notion_api_key = env::var("NOTION_API_KEY")?;
    let notion_database_id = env::var("NOTION_DATABASE_ID")?;
    let notion_url = "https://api.notion.com/v1/pages";

    let mut notion_headers = reqwest::header::HeaderMap::new();
    notion_headers.insert("Authorization", format!("Bearer {}", notion_api_key).parse()?);
    notion_headers.insert("Content-Type", "application/json".parse()?);
    notion_headers.insert("Notion-Version", "2022-06-28".parse()?);

    run_bidirectional_sync(&client, &client, &plex_url, &plex_token, notion_url, &notion_headers, &notion_database_id).await?;

    info!("CineLink shutting down...");
    Ok(())
}

