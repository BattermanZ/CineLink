mod models;
mod plex;
mod notion;
mod sync;
mod utils;
mod server;
mod tmdb;

use anyhow::{Result, Context};
use log::{info, debug};
use std::path::Path;

use crate::utils::{setup_logger, check_env_var};
use crate::server::start_server;

#[tokio::main]
async fn main() -> Result<()> {
    debug!("Starting main function");

    // Try to load .env from the current directory (for development)
    if dotenvy::dotenv().is_err() {
        // If that fails, try to load from /app/.env (for Docker)
        dotenvy::from_path(Path::new("/app/.env"))
            .context("Failed to load .env file from both current directory and /app/.env")?;
    }

    debug!("Setting up logger");
    setup_logger()?;

    info!("CineLink starting up...");

    let env_vars = [
        "NOTION_API_KEY",
        "NOTION_DATABASE_ID",
        "PLEX_URL",
        "PLEX_TOKEN",
        "API_KEY",
        "TMDB_API_KEY",
        "TVSHOWS_API_KEY",
    ];

    debug!("Checking environment variables");
    for var in env_vars.iter() {
        check_env_var(var)?;
    }

    debug!("Starting server");
    start_server().await?;

    debug!("Server started successfully");
    Ok(())
}

