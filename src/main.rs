use std::fs;
use log::{info, LevelFilter};
use env_logger::Builder;
use std::io::Write;
use chrono::{Local, Datelike};
use dotenvy::dotenv;
use std::env;
use anyhow::{Result, Context, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use regex::Regex;
use quick_xml::reader::Reader;
use quick_xml::events::Event;
use futures::future::join_all;
use tokio::task;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Movie {
    title: String,
    rating: f32,
}

async fn connect_to_plex(client: &Client, plex_url: &str, plex_token: &str) -> Result<()> {
    let url = format!("{}?X-Plex-Token={}", plex_url, plex_token);
    client.get(&url).send().await?;
    info!("Connected to Plex server successfully.");
    Ok(())
}

async fn get_all_movies(client: &Client, plex_url: &str, plex_token: &str) -> Result<Vec<Movie>> {
    let url = format!("{}/library/sections/1/all?X-Plex-Token={}", plex_url, plex_token);
    let response = client.get(&url).send().await?.text().await?;
    
    let mut reader = Reader::from_str(&response);

    let mut movies = Vec::new();
    let mut buf = Vec::new();
    let mut current_movie: Option<Movie> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if e.name().as_ref() == b"Video" {
                    let mut title = String::new();
                    let mut rating = 0.0;

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
                            _ => {}
                        }
                    }

                    if !title.is_empty() {
                        current_movie = Some(Movie { title, rating });
                    }
                }
            },
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"Video" {
                    if let Some(movie) = current_movie.take() {
                        movies.push(movie);
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error parsing XML: {:?}", e)),
            _ => {}
        }
        buf.clear();
    }

    let total_movies = movies.len();
    let rated_movies: Vec<Movie> = movies.into_iter()
        .filter(|movie| movie.rating > 0.0)
        .collect();

    info!("Retrieved {} movies from Plex, {} with ratings.", total_movies, rated_movies.len());
    for movie in &rated_movies {
        info!("Rated movie from Plex: {} | User Rating: {}", movie.title, movie.rating);
    }

    Ok(rated_movies)
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

async fn check_movie_exists(client: &Client, headers: &reqwest::header::HeaderMap, movie_title: &str, database_id: &str) -> Result<bool> {
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
        Ok(!result["results"].as_array().unwrap().is_empty())
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

async fn add_movies_in_batch(client: &Client, notion_url: &str, headers: &reqwest::header::HeaderMap, movie_list: &[Movie], database_id: &str) -> Result<String> {
    // First, check for existing movies in parallel
    let existing_movies_futures: Vec<_> = movie_list.iter()
        .map(|movie| {
            let client = client.clone();
            let headers = headers.clone();
            let database_id = database_id.to_string();
            let title = movie.title.clone();
            task::spawn(async move {
                match check_movie_exists(&client, &headers, &title, &database_id).await {
                    Ok(exists) => (title, exists),
                    Err(_) => (title, false), // Assume movie doesn't exist if there's an error
                }
            })
        })
        .collect();

    let existing_movies_results = join_all(existing_movies_futures).await;
    let existing_movies: Vec<String> = existing_movies_results.into_iter()
        .filter_map(|r| r.ok())
        .filter(|(_, exists)| *exists)
        .map(|(title, _)| title)
        .collect();

    let movies_to_add: Vec<&Movie> = movie_list.iter()
        .filter(|movie| !existing_movies.contains(&movie.title))
        .collect();

    if movies_to_add.is_empty() {
        info!("No new movies to add to Notion.");
        return Ok("No new movies to add.".to_string());
    }

    info!("Adding {} movies in batch to Notion...", movies_to_add.len());

    // Add movies in parallel
    let add_movie_futures: Vec<_> = movies_to_add.into_iter()
        .map(|movie| {
            let client = client.clone();
            let notion_url = notion_url.to_string();
            let headers = headers.clone();
            let database_id = database_id.to_string();
            let movie = movie.clone();
            task::spawn(async move {
                add_movie(&client, &notion_url, &headers, &movie, &database_id).await
            })
        })
        .collect();

    let results = join_all(add_movie_futures).await;
    let successful_additions = results.into_iter().filter(|r| r.as_ref().map_or(false, |inner_r| inner_r.is_ok())).count();

    Ok(format!("Batch processing completed. Successfully added {} movies.", successful_additions))
}

fn parse_logs() -> Result<(Option<(String, String)>, Vec<(String, String)>, String)> {
    let log_content = fs::read_to_string("logs/cinelink.log")?;
    let movie_addition_pattern = Regex::new(r"Movie '(.+?);' added to Notion with rating: '(.+?)'")?;
    let script_finished_pattern = Regex::new(r"Script finished at (.+)")?;

    let mut last_movie = None;
    let mut last_8_movies = Vec::new();
    let mut last_run_time = "No script run yet".to_string();

    for line in log_content.lines().rev() {
        if let Some(captures) = movie_addition_pattern.captures(line) {
            let movie_title = captures.get(1).unwrap().as_str().to_string();
            let movie_rating = captures.get(2).unwrap().as_str().to_string();
            
            if last_movie.is_none() {
                last_movie = Some((movie_title.clone(), movie_rating.clone()));
            }
            
            if last_8_movies.len() < 8 {
                last_8_movies.push((movie_title, movie_rating));
            }
        }

        if last_run_time == "No script run yet" {
            if let Some(captures) = script_finished_pattern.captures(line) {
                last_run_time = captures.get(1).unwrap().as_str().to_string();
            }
        }

        if last_movie.is_some() && last_8_movies.len() == 8 && last_run_time != "No script run yet" {
            break;
        }
    }

    Ok((last_movie, last_8_movies, last_run_time))
}

async fn run_script(client: &Client, plex_url: &str, plex_token: &str, notion_url: &str, headers: &reqwest::header::HeaderMap, database_id: &str) -> Result<()> {
    info!("Starting script...");

    connect_to_plex(client, plex_url, plex_token).await?;

    let movies = get_all_movies(client, plex_url, plex_token).await?;
    info!("Found {} rated movies in your Plex library.", movies.len());

    info!("Attempting to add rated movies to Notion...");
    let response = add_movies_in_batch(client, notion_url, headers, &movies, database_id).await?;
    info!("Notion API Response: {}", response);

    info!("Script finished at {}", Local::now());
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

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", format!("Bearer {}", notion_api_key).parse()?);
    headers.insert("Content-Type", "application/json".parse()?);
    headers.insert("Notion-Version", "2022-06-28".parse()?);

    run_script(&client, &plex_url, &plex_token, notion_url, &headers, &notion_database_id).await?;

    info!("CineLink shutting down...");
    Ok(())
}

