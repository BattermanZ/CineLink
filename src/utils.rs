use anyhow::{Result, Context, anyhow};
use chrono::Local;
use env_logger::Builder;
use log::{info, LevelFilter};
use std::env;
use std::fs;
use std::io::Write;

pub fn numeric_to_emoji_rating(numeric_rating: f32) -> &'static str {
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

pub fn setup_logger() -> Result<()> {
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

pub fn check_env_var(var_name: &str) -> Result<()> {
    match env::var(var_name) {
        Ok(_) => {
            info!("Environment variable '{}' found.", var_name);
            Ok(())
        },
        Err(_) => Err(anyhow!("Environment variable '{}' not found. Please set it in your .env file.", var_name)),
    }
}

