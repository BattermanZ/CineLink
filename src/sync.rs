use anyhow::Result;
use log::{info, error};
use reqwest::Client;

use crate::models::Movie;
use crate::plex::{get_all_movies, update_plex_rating};
use crate::notion::{get_all_notion_movies, add_movies_in_batch};

pub async fn sync_notion_to_plex(notion_client: &Client, plex_client: &Client, notion_headers: &reqwest::header::HeaderMap, notion_db_id: &str, plex_url: &str, plex_token: &str, plex_movies: &[Movie]) -> Result<()> {
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

pub async fn sync_plex_to_notion(notion_client: &Client, notion_url: &str, notion_headers: &reqwest::header::HeaderMap, notion_db_id: &str, plex_movies: &[Movie]) -> Result<()> {
    info!("Starting Plex to Notion sync");

    let response = add_movies_in_batch(notion_client, notion_url, notion_headers, plex_movies, notion_db_id).await?;
    info!("Plex to Notion sync completed: {}", response);
    Ok(())
}

pub async fn run_bidirectional_sync(
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

    Ok(())
}

