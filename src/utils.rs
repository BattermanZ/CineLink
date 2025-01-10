use anyhow::{Result, Context, anyhow};
use log::{info, LevelFilter};
use log4rs::append::console::ConsoleAppender;
use log4rs::append::file::FileAppender;
use log4rs::encode::pattern::PatternEncoder;
use log4rs::config::{Appender, Config, Root};
use std::env;
use std::fs;

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

    let log_pattern = "{d(%Y-%m-%d %H:%M:%S)} [{l}] - {m}{n}";

    // Create stdout appender
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(log_pattern)))
        .build();

    // Create file appender
    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(log_pattern)))
        .build("logs/cinelink.log")?;

    // Build config with both appenders
    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder()
            .appender("stdout")
            .appender("logfile")
            .build(LevelFilter::Info))?;

    // Initialize the logger
    log4rs::init_config(config)?;

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

