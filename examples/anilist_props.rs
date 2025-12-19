use anyhow::Result;
use cinelink::anilist::{AniListClient, AniListMediaType};
use dotenvy::dotenv;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn usage() -> ! {
    eprintln!("Usage: cargo run --example anilist_props -- <anime|manga> <anilist_id>");
    std::process::exit(2);
}

fn parse_args() -> (AniListMediaType, i32) {
    let mut args = env::args().skip(1);
    let kind = args.next().unwrap_or_else(|| "anime".to_string());
    let id = args.next().unwrap_or_else(|| usage());
    let id: i32 = id.parse().unwrap_or_else(|_| usage());
    let media_type = match kind.as_str() {
        "anime" => AniListMediaType::Anime,
        "manga" => AniListMediaType::Manga,
        _ => usage(),
    };
    (media_type, id)
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv();
    init_tracing();

    let (media_type, id) = parse_args();
    info!("AniList fetch: {:?} id={}", media_type, id);

    let client = AniListClient::new()?;
    let mapped = client.fetch_mapped(media_type, id).await?;

    info!("--- AniList mapped fields ---");
    info!("id: {}", mapped.id);
    info!("id_mal: {:?}", mapped.id_mal);
    info!("name: {}", mapped.name);
    if let Some(v) = &mapped.eng_name {
        info!("eng_name: {}", v);
    }
    if let Some(v) = &mapped.original_title {
        info!("original_title: {}", v);
    }
    info!("is_adult: {}", mapped.is_adult);
    info!("content_rating: {}", mapped.content_rating);
    info!(
        "country_of_origin: {}",
        mapped.country_of_origin.as_deref().unwrap_or("<none>")
    );
    info!(
        "language: {}",
        mapped.language.as_deref().unwrap_or("<none>")
    );
    info!(
        "release_date: {}",
        mapped
            .release_date
            .as_deref()
            .unwrap_or("<partial/unknown>")
    );
    info!("year: {}", mapped.year.as_deref().unwrap_or("<none>"));
    info!("episodes: {:?}", mapped.episodes);
    info!("runtime_minutes: {:?}", mapped.runtime_minutes);
    info!("genres: {:?}", mapped.genres);
    info!("director: {:?}", mapped.director);
    info!("cast: {:?}", mapped.cast);
    info!("trailer: {}", mapped.trailer.as_deref().unwrap_or("<none>"));
    info!("poster: {}", mapped.poster.as_deref().unwrap_or("<none>"));
    info!(
        "backdrop: {}",
        mapped.backdrop.as_deref().unwrap_or("<none>")
    );
    info!(
        "imdb_page: {}",
        mapped.imdb_page.as_deref().unwrap_or("<none>")
    );

    if let Some(desc) = mapped.synopsis {
        info!(
            "synopsis (first 280 chars): {}",
            desc.chars().take(280).collect::<String>()
        );
    } else {
        info!("synopsis: <none>");
    }

    Ok(())
}
