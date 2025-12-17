use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use chrono::Utc;
use cinelink::anilist::{AniListApi, AniListMapped};
use cinelink::app::{build_router, AppState};
use cinelink::notion::{NotionApi, PropertySchema, PropertyType, NOTION_VERSION};
use cinelink::tmdb::{MediaData, TmdbApi};
use hmac::{Hmac, Mac};
use serde_json::{json, Map, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::util::ServiceExt;

const WEBHOOK_SECRET: &str = "test-secret";

struct FakeNotion {
    schema: PropertySchema,
    pages: Mutex<HashMap<String, Value>>,
    updates: Mutex<Vec<(String, Map<String, Value>, Option<Value>, Option<Value>)>>,
}

#[async_trait::async_trait]
impl NotionApi for FakeNotion {
    async fn fetch_property_schema(&self) -> anyhow::Result<PropertySchema> {
        Ok(self.schema.clone())
    }

    async fn fetch_page(&self, page_id: &str) -> anyhow::Result<Value> {
        self.pages
            .lock()
            .unwrap()
            .get(page_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing page {}", page_id))
    }

    async fn update_page(
        &self,
        page_id: &str,
        properties: Map<String, Value>,
        _icon: Option<Value>,
        _cover: Option<Value>,
    ) -> anyhow::Result<()> {
        self.updates
            .lock()
            .unwrap()
            .push((page_id.to_string(), properties, _icon, _cover));
        Ok(())
    }
}

struct FakeTmdb {
    movie: MediaData,
    tv: MediaData,
}

#[async_trait::async_trait]
impl TmdbApi for FakeTmdb {
    async fn search_movie(&self, _query: &str) -> anyhow::Result<i32> {
        Ok(self.movie.id)
    }
    async fn search_tv(&self, _query: &str) -> anyhow::Result<i32> {
        Ok(self.tv.id)
    }
    async fn resolve_movie_id(&self, query: &str) -> anyhow::Result<i32> {
        if query == "tt12345" {
            return Ok(self.movie.id);
        }
        self.search_movie(query).await
    }
    async fn resolve_tv_id(&self, query: &str) -> anyhow::Result<i32> {
        if query == "tt99999" {
            return Ok(self.tv.id);
        }
        self.search_tv(query).await
    }
    async fn lookup_imdb(&self, imdb_id: &str) -> anyhow::Result<(Option<i32>, Option<i32>)> {
        match imdb_id {
            "tt12345" => Ok((Some(self.movie.id), None)),
            "tt99999" => Ok((None, Some(self.tv.id))),
            _ => Ok((None, None)),
        }
    }
    async fn fetch_movie(&self, id: i32) -> anyhow::Result<MediaData> {
        assert_eq!(id, self.movie.id);
        Ok(self.movie.clone())
    }
    async fn fetch_tv_season(&self, id: i32, _season: i32) -> anyhow::Result<MediaData> {
        assert_eq!(id, self.tv.id);
        Ok(self.tv.clone())
    }
}

struct FakeAniList {
    resolved_id: i32,
    anime: AniListMapped,
}

#[async_trait::async_trait]
impl AniListApi for FakeAniList {
    async fn resolve_anime_id(&self, _query: &str, season: Option<i32>) -> anyhow::Result<i32> {
        assert_eq!(season, Some(2));
        Ok(self.resolved_id)
    }

    async fn fetch_anime(&self, id: i32) -> anyhow::Result<AniListMapped> {
        assert_eq!(id, self.resolved_id);
        Ok(self.anime.clone())
    }
}

fn base_schema() -> PropertySchema {
    let mut types = HashMap::new();
    types.insert("Name".to_string(), PropertyType::Title);
    types.insert("Eng Name".to_string(), PropertyType::RichText);
    types.insert("Original Title".to_string(), PropertyType::RichText);
    types.insert("Synopsis".to_string(), PropertyType::RichText);
    types.insert("Genre".to_string(), PropertyType::MultiSelect);
    types.insert("Cast".to_string(), PropertyType::RichText);
    types.insert("Director".to_string(), PropertyType::RichText);
    types.insert("Content Rating".to_string(), PropertyType::Select);
    types.insert("Country of origin".to_string(), PropertyType::RichText);
    types.insert("Language".to_string(), PropertyType::Select);
    types.insert("Release Date".to_string(), PropertyType::Date);
    types.insert("Year".to_string(), PropertyType::RichText);
    types.insert("Runtime".to_string(), PropertyType::Number);
    types.insert("Episodes".to_string(), PropertyType::Number);
    types.insert("Trailer".to_string(), PropertyType::Url);
    types.insert("IMG".to_string(), PropertyType::Files);
    types.insert("IMDb Page".to_string(), PropertyType::Url);
    types.insert("ID".to_string(), PropertyType::Number);
    types.insert("Season".to_string(), PropertyType::Select);
    types.insert("Type".to_string(), PropertyType::Select);
    PropertySchema {
        types,
        title_property: Some("Name".to_string()),
    }
}

fn make_page(raw_title: &str, type_select: &str, season: Option<&str>) -> Value {
    let mut props = Map::new();
    props.insert(
        "Name".to_string(),
        json!({
            "title": [{
                "text": { "content": raw_title },
                "plain_text": raw_title
            }]
        }),
    );
    props.insert(
        "Type".to_string(),
        json!({
            "select": { "name": type_select }
        }),
    );
    props.insert(
        "ID".to_string(),
        json!({
            "number": null
        }),
    );
    if let Some(season_name) = season {
        props.insert(
            "Season".to_string(),
            json!({
                "select": { "name": season_name }
            }),
        );
    }

    json!({
        "id": "page-1",
        "properties": props
    })
}

fn tmdb_movie() -> MediaData {
    MediaData {
        id: 101,
        name: "TMDB Movie".to_string(),
        eng_name: Some("TMDB Movie".to_string()),
        synopsis: Some("Movie overview".to_string()),
        genres: vec!["Drama".to_string()],
        cast: vec!["Actor A".to_string()],
        director: vec!["Director A".to_string()],
        content_rating: Some("PG-13".to_string()),
        country_of_origin: vec!["US".to_string()],
        language: Some("English".to_string()),
        original_language: "en".to_string(),
        release_date: Some("2024-01-01".to_string()),
        year: Some("2024".to_string()),
        runtime_minutes: Some(120.0),
        episodes: None,
        trailer: Some("https://youtube.com/movie".to_string()),
        poster: Some("https://image.tmdb.org/movie.jpg".to_string()),
        backdrop: None,
        imdb_page: Some("https://imdb.com/title/tt123".to_string()),
    }
}

fn tmdb_tv() -> MediaData {
    MediaData {
        id: 202,
        name: "TMDB Show".to_string(),
        eng_name: Some("TMDB Show".to_string()),
        synopsis: Some("Show overview".to_string()),
        genres: vec!["Sci-Fi".to_string()],
        cast: vec!["Actor B".to_string()],
        director: vec!["Creator B".to_string()],
        content_rating: Some("TV-MA".to_string()),
        country_of_origin: vec!["US".to_string()],
        language: Some("English".to_string()),
        original_language: "en".to_string(),
        release_date: Some("2025-02-02".to_string()),
        year: Some("2025".to_string()),
        runtime_minutes: Some(45.0),
        episodes: Some(8),
        trailer: Some("https://youtube.com/show".to_string()),
        poster: Some("https://image.tmdb.org/show.jpg".to_string()),
        backdrop: None,
        imdb_page: Some("https://imdb.com/title/tt456".to_string()),
    }
}

fn app_with_mocks(page: Value, tmdb: FakeTmdb) -> (Router, Arc<FakeNotion>) {
    let schema = base_schema();
    let notion = Arc::new(FakeNotion {
        schema: schema.clone(),
        pages: Mutex::new(HashMap::from([(
            page.get("id").unwrap().as_str().unwrap().to_string(),
            page,
        )])),
        updates: Mutex::new(Vec::new()),
    });

    let state = AppState {
        notion: notion.clone(),
        tmdb: Arc::new(tmdb),
        anilist: Arc::new(FakeAniList {
            resolved_id: 176496,
            anime: AniListMapped {
                id: 176496,
                id_mal: None,
                name: "AniList English Season 2".to_string(),
                eng_name: None,
                original_title: Some("AniList Romaji Season 2".to_string()),
                synopsis: Some("AniList synopsis".to_string()),
                genres: vec!["Action".to_string()],
                cast: vec!["Cast A".to_string()],
                director: vec!["Director A".to_string()],
                is_adult: false,
                content_rating: "All Audiences".to_string(),
                country_of_origin: Some("Japan".to_string()),
                language: Some("Japanese".to_string()),
                release_date: Some("2025-01-05".to_string()),
                year: Some("2025".to_string()),
                runtime_minutes: Some(24.0),
                episodes: Some(13),
                trailer: Some("https://youtube.com/anime".to_string()),
                poster: Some("https://anilist/poster.png".to_string()),
                backdrop: Some("https://anilist/backdrop.jpg".to_string()),
                imdb_page: Some("https://anilist.co/anime/176496".to_string()),
            },
        }),
        title_property: "Name".to_string(),
        schema: Arc::new(schema),
        signing_secret: WEBHOOK_SECRET.to_string(),
        rate_limits: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        global_limit: Arc::new(tokio::sync::Mutex::new(cinelink::app::WindowCounter {
            window: 0,
            count: 0,
        })),
        recent_events: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        processing_sem: Arc::new(tokio::sync::Semaphore::new(8)),
    };

    (build_router(state), notion)
}

fn webhook_payload(updated: &[&str], page_id: &str) -> String {
    let id = format!(
        "evt-{}-{}",
        page_id,
        updated.iter().copied().collect::<Vec<_>>().join(",")
    );
    json!({
        "id": id,
        "timestamp": Utc::now().to_rfc3339(),
        "type": "page.properties_updated",
        "entity": { "id": page_id, "type": "page" },
        "data": {
            "updated_properties": updated
        }
    })
    .to_string()
}

async fn wait_for_update_count(notion: &Arc<FakeNotion>, expected: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if notion.updates.lock().unwrap().len() >= expected {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for {} updates (got {})",
                expected,
                notion.updates.lock().unwrap().len()
            );
        }
        tokio::task::yield_now().await;
    }
}

async fn assert_no_updates(notion: &Arc<FakeNotion>) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(notion.updates.lock().unwrap().is_empty());
}

fn sign_body(body: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).expect("static key is valid");
    mac.update(body.as_bytes());
    let digest = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(digest))
}

fn signed_request(body: String) -> Request<Body> {
    Request::post("/")
        .header("content-type", "application/json")
        .header("Notion-Version", NOTION_VERSION)
        .header("x-notion-signature", sign_body(&body))
        .body(Body::from(body))
        .expect("failed to build request")
}

#[tokio::test]
async fn ignores_when_title_has_no_semicolon() {
    let page = make_page("Movie Title", "Movie", None);
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: tmdb_movie(),
            tv: tmdb_tv(),
        },
    );

    let payload = webhook_payload(&["title"], page.get("id").unwrap().as_str().unwrap());
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_no_updates(&notion).await;
}

#[tokio::test]
async fn updates_movie_when_title_has_semicolon() {
    let page = make_page("Movie Title ;", "Movie", None);
    let movie = tmdb_movie();
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: movie.clone(),
            tv: tmdb_tv(),
        },
    );

    let payload = webhook_payload(&["title"], page.get("id").unwrap().as_str().unwrap());
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    wait_for_update_count(&notion, 1).await;
    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props, icon, cover) = &updates[0];
    let name = props
        .get("Name")
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|s| s.as_str());
    assert_eq!(name, Some(movie.name.as_str()));
    let imdb = props
        .get("IMDb Page")
        .and_then(|p| p.get("url"))
        .and_then(|v| v.as_str());
    assert_eq!(imdb, movie.imdb_page.as_deref());
    let icon_url = icon
        .as_ref()
        .and_then(|i| i.get("external"))
        .and_then(|e| e.get("url"))
        .and_then(|u| u.as_str());
    assert_eq!(icon_url, movie.poster.as_deref());
    let cover_url = cover
        .as_ref()
        .and_then(|c| c.get("external"))
        .and_then(|e| e.get("url"))
        .and_then(|u| u.as_str());
    assert!(cover_url.is_none()); // movie fixture has no backdrop
}

#[tokio::test]
async fn ignores_tv_without_season() {
    let page = make_page("Show Title ;", "TV", None);
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: tmdb_movie(),
            tv: tmdb_tv(),
        },
    );

    let payload = webhook_payload(&["season"], page.get("id").unwrap().as_str().unwrap());
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_no_updates(&notion).await;
}

#[tokio::test]
async fn updates_tv_with_season() {
    let page = make_page("Show Title ;", "TV", Some("Season 1"));
    let tv = tmdb_tv();
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: tmdb_movie(),
            tv: tv.clone(),
        },
    );

    let payload = webhook_payload(
        &["title", "season"],
        page.get("id").unwrap().as_str().unwrap(),
    );
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    wait_for_update_count(&notion, 1).await;
    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props, icon, cover) = &updates[0];
    let name = props
        .get("Name")
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|s| s.as_str());
    assert_eq!(name, Some(tv.name.as_str()));
    let episodes = props
        .get("Episodes")
        .and_then(|p| p.get("number"))
        .and_then(|v| v.as_f64());
    assert_eq!(episodes, tv.episodes.map(|e| e as f64));
    let icon_url = icon
        .as_ref()
        .and_then(|i| i.get("external"))
        .and_then(|e| e.get("url"))
        .and_then(|u| u.as_str());
    assert_eq!(icon_url, tv.poster.as_deref());
    let cover_url = cover
        .as_ref()
        .and_then(|c| c.get("external"))
        .and_then(|e| e.get("url"))
        .and_then(|u| u.as_str());
    assert!(cover_url.is_none()); // tv fixture has no backdrop
}

#[tokio::test]
async fn resolves_imdb_id_for_movie() {
    let page = make_page("tt12345 ;", "Movie", None);
    let movie = tmdb_movie();
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: movie.clone(),
            tv: tmdb_tv(),
        },
    );

    let payload = webhook_payload(&["title"], page.get("id").unwrap().as_str().unwrap());
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    wait_for_update_count(&notion, 1).await;
    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props, _, _) = &updates[0];
    let name = props
        .get("Name")
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|s| s.as_str());
    assert_eq!(name, Some(movie.name.as_str()));
}

#[tokio::test]
async fn resolves_imdb_id_for_tv_even_if_type_movie() {
    // Type is Movie but imdb points to a TV show; resolver should switch to TV and succeed.
    let page = make_page("tt99999 ;", "Movie", Some("Season 1"));
    let tv = tmdb_tv();
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: tmdb_movie(),
            tv: tv.clone(),
        },
    );

    let payload = webhook_payload(
        &["title", "season"],
        page.get("id").unwrap().as_str().unwrap(),
    );
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    wait_for_update_count(&notion, 1).await;
    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props, _, _) = &updates[0];
    let name = props
        .get("Name")
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|s| s.as_str());
    assert_eq!(name, Some(tv.name.as_str()));
}

#[tokio::test]
async fn updates_anime_when_title_has_equals() {
    let page = make_page("Ani Query=", "tv", Some("Season 2"));
    let tmdb = FakeTmdb {
        movie: tmdb_movie(),
        tv: tmdb_tv(),
    };
    let (app, notion) = app_with_mocks(page, tmdb);

    let page_id = "page-1";
    let body = webhook_payload(&["title"], page_id);
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("Content-Type", "application/json")
        .header("Notion-Version", NOTION_VERSION)
        .header("x-notion-signature", sign_body(&body))
        .body(Body::from(body))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    wait_for_update_count(&notion, 1).await;
    let updates = notion.updates.lock().unwrap();
    let (_, props, _icon, _cover) = updates.last().unwrap();

    let name = props
        .get("Name")
        .and_then(|v| v.get("title"))
        .and_then(|arr| arr.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|v| v.as_str());
    assert_eq!(name, Some("AniList English"));

    let original = props
        .get("Original Title")
        .and_then(|v| v.get("rich_text"))
        .and_then(|arr| arr.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|v| v.as_str());
    assert_eq!(original, Some("AniList Romaji"));

    let eng_name = props
        .get("Eng Name")
        .and_then(|v| v.get("rich_text"))
        .and_then(|arr| arr.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|v| v.as_str());
    assert_eq!(eng_name, Some(""));

    let id = props
        .get("ID")
        .and_then(|v| v.get("number"))
        .and_then(|v| v.as_f64());
    assert_eq!(id, Some(176496.0));

    let imdb_page = props
        .get("IMDb Page")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str());
    assert_eq!(imdb_page, Some("https://anilist.co/anime/176496"));
}

#[tokio::test]
async fn uses_original_title_for_french_with_eng_name_set() {
    // Simulate French media: original title should be used for Name; Eng Name should be set.
    let french_media = MediaData {
        id: 303,
        name: "Titre anglais".to_string(), // localized title
        eng_name: None,                    // will be ignored; we set below
        synopsis: None,
        genres: vec![],
        cast: vec![],
        director: vec![],
        content_rating: None,
        country_of_origin: vec![],
        language: Some("French".to_string()),
        original_language: "fr".to_string(),
        release_date: None,
        year: None,
        runtime_minutes: None,
        episodes: None,
        trailer: None,
        poster: None,
        backdrop: None,
        imdb_page: None,
    };
    let french_media_with_titles = MediaData {
        name: "Titre original".to_string(),
        eng_name: Some("English Title".to_string()),
        ..french_media
    };

    let page = make_page("Titre original ;", "Movie", None);
    let (app, notion) = app_with_mocks(
        page.clone(),
        FakeTmdb {
            movie: french_media_with_titles.clone(),
            tv: tmdb_tv(),
        },
    );

    let payload = webhook_payload(&["title"], page.get("id").unwrap().as_str().unwrap());
    let res = app.oneshot(signed_request(payload)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    wait_for_update_count(&notion, 1).await;
    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props, _, _) = &updates[0];

    let name = props
        .get("Name")
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|s| s.as_str());
    assert_eq!(name, Some("Titre original"));

    let eng_name = props
        .get("Eng Name")
        .and_then(|p| p.get("rich_text"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|s| s.as_str());
    assert_eq!(eng_name, Some("English Title"));
}
