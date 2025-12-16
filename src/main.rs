use anyhow::Result;
use dotenvy::dotenv;
use std::env;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn check_env() -> Result<()> {
    let required = [
        "NOTION_API_KEY",
        "NOTION_DATABASE_ID",
        "TMDB_API_KEY",
        "NOTION_WEBHOOK_SECRET",
    ];
    for key in required {
        if env::var(key).is_err() {
            anyhow::bail!("Missing required environment variable: {}", key);
        }
    }
    info!("All required environment variables are set");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load `.env` before initializing tracing so `RUST_LOG` (if present) is applied.
    let dotenv_result = dotenv();
    init_tracing();
    match dotenv_result {
        Ok(path) => info!("Loaded environment from {:?}", path),
        Err(e) => warn!("No .env file loaded ({}) - relying on environment", e),
    }
    check_env()?;
    cinelink::app::run_server().await
}
