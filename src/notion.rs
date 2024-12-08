use anyhow::{Result, anyhow};
use chrono::{Local, Datelike};
use log::{info, error};
use reqwest::Client;
use serde_json::json;
use tokio::task;
use futures::future::join_all;

use crate::models::Movie;
use crate::utils::numeric_to_emoji_rating;

pub async fn get_notion_movie_info(client: &Client, headers: &reqwest::header::HeaderMap, movie_title: &str, database_id: &str) -> Result<(bool, Option<String>, bool)> {
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

pub async fn add_movie(client: &Client, notion_url: &str, headers: &reqwest::header::HeaderMap, movie: &Movie, database_id: &str) -> Result<()> {
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

pub async fn update_movie_rating_in_notion(client: &Client, headers: &reqwest::header::HeaderMap, page_id: &str, movie_title: &str, rating: f32) -> Result<()> {
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

pub async fn add_movies_in_batch(client: &Client, notion_url: &str, headers: &reqwest::header::HeaderMap, movie_list: &[Movie], database_id: &str) -> Result<String> {
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

pub async fn get_all_notion_movies(client: &Client, headers: &reqwest::header::HeaderMap, database_id: &str) -> Result<Vec<Movie>> {
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

