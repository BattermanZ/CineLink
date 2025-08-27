use anyhow::{Result, anyhow};
use chrono::{Local, Datelike};
use log::{info, error, debug};
use reqwest::Client;
use serde_json::json;
use tokio::task;
use futures::future::join_all;

use crate::models::{Movie, TvShow, TvSeason};
use crate::utils::numeric_to_emoji_rating;
use crate::tmdb::{get_tv_season_details, search_tv_show, TvSeasonDetails};

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

#[allow(dead_code)]
pub async fn add_tv_season(
    client: &Client,
    notion_url: &str,
    headers: &reqwest::header::HeaderMap,
    season: &TvSeason,
    database_id: &str
) -> Result<()> {
    let season_title = format!("{} - Season {}", season.show_title, season.season_number);
    let rating_emoji = season.rating.map(numeric_to_emoji_rating);
    let current_year = Local::now().year().to_string();

    let mut properties = json!({
        "parent": {"database_id": database_id},
        "properties": {
            "Name": {
                "title": [
                    {"text": {"content": format!("{};", season_title)}}
                ]
            },
            "Type": {
                "select": {"name": "TV Show"}
            },
            "Overview": {
                "rich_text": [
                    {"text": {"content": season.overview}}
                ]
            },
            "Years watched": {
                "multi_select": [{"name": current_year}]
            },
            "Cast": {
                "multi_select": season.cast.iter().map(|name| json!({"name": name})).collect::<Vec<_>>()
            }
        }
    });

    // Add optional properties
    if let Some(rating) = rating_emoji {
        properties["properties"]["Aurel's rating"] = json!({"select": {"name": rating}});
    }
    
    if let Some(poster_url) = &season.poster_url {
        properties["properties"]["Poster"] = json!({"url": poster_url});
    }

    if let Some(air_date) = &season.air_date {
        properties["properties"]["Release Date"] = json!({"date": {"start": air_date}});
    }

    if let Some(trailer_url) = &season.trailer_url {
        properties["properties"]["Trailer"] = json!({"url": trailer_url});
    }

    let response = client.post(notion_url)
        .headers(headers.clone())
        .json(&properties)
        .send()
        .await?;

    if response.status().is_success() {
        info!("TV Season '{}' added to Notion", season_title);
        Ok(())
    } else {
        Err(anyhow!("Failed to add '{}' to Notion. Status: {}", season_title, response.status()))
    }
}

#[allow(dead_code)]
pub async fn update_tv_season_with_tmdb_data(
    client: &Client,
    notion_url: &str,
    headers: &reqwest::header::HeaderMap,
    tv_show: &TvShow,
    database_id: &str
) -> Result<()> {
    // First, search for the TV show on TMDB
    let tmdb_id = match tv_show.tmdb_id {
        Some(id) => id,
        None => search_tv_show(client, &tv_show.title).await?,
    };

    // Get season details from TMDB
    let season_details = get_tv_season_details(client, tmdb_id, tv_show.season_number).await?;

    // Find trailer URL (prefer YouTube trailers)
    let trailer_url = season_details.videos
        .and_then(|videos| {
            videos.results
                .iter()
                .find(|v| v.site == "YouTube" && v.video_type == "Trailer")
                .map(|v| format!("https://www.youtube.com/watch?v={}", v.key))
        });

    // Create poster URL if available
    let poster_url = season_details.poster_path
        .map(|path| format!("https://image.tmdb.org/t/p/original{}", path));

    // Extract cast names (limit to main cast)
    let cast = season_details.credits
        .map(|credits| {
            credits.cast
                .iter()
                .take(10)  // Limit to top 10 cast members
                .map(|member| member.name.clone())
                .collect()
        })
        .unwrap_or_default();

    // Create TvSeason object
    let season = TvSeason {
        show_title: tv_show.title.clone(),
        season_number: tv_show.season_number,
        overview: season_details.overview,
        poster_url,
        air_date: season_details.air_date,
        cast,
        trailer_url,
        rating: Some(tv_show.rating),
        rating_key: tv_show.rating_key.clone(),
        library_id: tv_show.library_id.clone(),
    };

    // Add to Notion
    add_tv_season(client, notion_url, headers, &season, database_id).await
}

pub async fn update_tv_shows_with_tmdb(
    client: &Client,
    _notion_url: &str,
    headers: &reqwest::header::HeaderMap,
    database_id: &str,
) -> Result<()> {
    // Query for TV Series entries
    let query_url = format!("https://api.notion.com/v1/databases/{}/query", database_id);
    let query_payload = json!({
        "filter": {
            "property": "Type",
            "select": {
                "equals": "TV Series"
            }
        }
    });

    let response = client.post(&query_url)
        .headers(headers.clone())
        .json(&query_payload)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to query Notion database. Status: {}", response.status()));
    }

    let result: serde_json::Value = response.json().await?;
    let pages = result["results"].as_array()
        .ok_or_else(|| anyhow!("Unexpected response format from Notion"))?;

    for page in pages {
        let page_id = page["id"].as_str()
            .ok_or_else(|| anyhow!("Missing page ID"))?;
        
        let title = page["properties"]["Name"]["title"][0]["text"]["content"].as_str()
            .ok_or_else(|| anyhow!("Missing title"))?
            .trim_end_matches(';')
            .to_string();

        // Skip if Season property is not set
        let season_select = page["properties"]["Season"]["select"].as_object();
        if season_select.is_none() || season_select.unwrap().is_empty() {
            debug!("Skipping {} - Season property not set", title);
            continue;
        }

        // Get the Season property value
        let season_str = match page["properties"]["Season"]["select"]["name"].as_str() {
            Some(s) => s,
            None => {
                error!("Missing Season name for {}", title);
                continue;
            }
        };

        // Parse season number
        let season_number = match season_str {
            "Mini-series" => 1,
            s if s.starts_with("Season ") => {
                match s.trim_start_matches("Season ").parse::<i32>() {
                    Ok(num) => num,
                    Err(_) => {
                        error!("Invalid season format for {}: {}", title, s);
                        continue;
                    }
                }
            },
            _ => {
                error!("Unexpected season format for {}: {}", title, season_str);
                continue;
            }
        };

        info!("Updating TV show: {} - {}", title, season_str);

        // Search for show and get TMDB data
        match search_tv_show(client, &title).await {
            Ok(tmdb_id) => {
                debug!("Found TMDB ID {} for show {}", tmdb_id, title);
                match get_tv_season_details(client, tmdb_id, season_number).await {
                    Ok(season_details) => {
                        match update_notion_tv_show_page(
                            client,
                            headers,
                            page_id,
                            &season_details,
                        ).await {
                            Ok(_) => info!("Successfully updated {}", title),
                            Err(e) => error!("Failed to update Notion page for {}: {}", title, e),
                        }
                    }
                    Err(e) => error!("Failed to get TMDB details for {}: {}", title, e),
                }
            }
            Err(e) => error!("Failed to find TV show on TMDB: {}: {}", title, e),
        }
    }

    Ok(())
}

async fn update_notion_tv_show_page(
    client: &Client,
    headers: &reqwest::header::HeaderMap,
    page_id: &str,
    season_details: &TvSeasonDetails,
) -> Result<()> {
    let notion_page_url = format!("https://api.notion.com/v1/pages/{}", page_id);

    // Create cast list as comma-separated string
    let cast_text = season_details.credits.as_ref()
        .map(|credits| {
            credits.cast.iter()
                .take(10)
                .map(|member| member.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    // Find trailer (looking specifically for YouTube trailers)
    let trailer_url = season_details.videos.as_ref()
        .and_then(|videos| {
            videos.results.iter()
                .find(|v| v.site == "YouTube" && v.video_type == "Trailer")
                .map(|v| format!("https://www.youtube.com/watch?v={}", v.key))
        });

    // Extract year from air date
    let year = season_details.air_date
        .as_ref()
        .and_then(|date| date.split('-').next())
        .unwrap_or("").to_string();

    // Create page properties update
    let mut properties = json!({
        "Synopsis": {
            "rich_text": [
                {"text": {"content": season_details.overview}}
            ]
        },
        "Cast": {
            "rich_text": [
                {"text": {"content": cast_text}}
            ]
        },
        "Year": {
            "rich_text": [
                {"text": {"content": year}}
            ]
        }
    });

    if let Some(url) = trailer_url {
        properties["Trailer"] = json!({"url": url});
    }

    // Create update payload
    let mut update_payload = json!({
        "properties": properties
    });

    // Add cover and icon if poster is available
    if let Some(poster_path) = &season_details.poster_path {
        let poster_url = format!("https://image.tmdb.org/t/p/original{}", poster_path);
        
        // Set cover image
        update_payload["cover"] = json!({
            "type": "external",
            "external": {
                "url": poster_url.clone()
            }
        });

        // Set page icon
        update_payload["icon"] = json!({
            "type": "external",
            "external": {
                "url": poster_url
            }
        });
    }

    debug!("Updating Notion page with payload: {}", serde_json::to_string_pretty(&update_payload)?);

    let response = client.patch(&notion_page_url)
        .headers(headers.clone())
        .json(&update_payload)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await?;
        error!("Failed to update Notion page. Status: {}, Response: {}", status, error_text);
        return Err(anyhow!("Failed to update Notion page. Status: {}", status));
    }

    Ok(())
}

