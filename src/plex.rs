use anyhow::{Result, anyhow};
use log::{info, debug};
use quick_xml::reader::Reader;
use quick_xml::events::Event;
use reqwest::Client;

use crate::models::Movie;

pub async fn get_all_movies(client: &Client, plex_url: &str, plex_token: &str) -> Result<(Vec<Movie>, Vec<Movie>)> {
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

pub async fn update_plex_rating(client: &Client, plex_url: &str, plex_token: &str, notion_movie: &Movie, plex_movies: &[Movie]) -> Result<()> {
    if let Some(plex_movie) = plex_movies.iter().find(|m| m.title == notion_movie.title) {
        if (plex_movie.rating - notion_movie.rating).abs() < f32::EPSILON {
            debug!("Rating for '{}' is already {} in both Notion and Plex. No update needed.", notion_movie.title, notion_movie.rating);
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

