mod models;
mod plex;
mod notion;
mod sync;
mod utils;
mod server;

use anyhow::{Context, Result};
use dotenvy::dotenv;
use log::info;

use crate::utils::{setup_logger, check_env_var};
use crate::server::start_server;

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
        "API_KEY",
    ];

    for var in env_vars.iter() {
        check_env_var(var)?;
    }

    start_server().await?;

    Ok(())
}

