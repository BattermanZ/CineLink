use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cinelink::app::{build_router, AppState};
use cinelink::notion::{NotionApi, PropertySchema, PropertyType};
use cinelink::tmdb::{MediaData, TmdbApi};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::util::ServiceExt;

struct FakeNotion {
    schema: PropertySchema,
    pages: Mutex<HashMap<String, Value>>,
    updates: Mutex<Vec<(String, Map<String, Value>)>>,
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
    ) -> anyhow::Result<()> {
        self.updates
            .lock()
            .unwrap()
            .push((page_id.to_string(), properties));
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
    async fn fetch_movie(&self, id: i32) -> anyhow::Result<MediaData> {
        assert_eq!(id, self.movie.id);
        Ok(self.movie.clone())
    }
    async fn fetch_tv_season(&self, id: i32, _season: i32) -> anyhow::Result<MediaData> {
        assert_eq!(id, self.tv.id);
        Ok(self.tv.clone())
    }
}

fn base_schema() -> PropertySchema {
    let mut types = HashMap::new();
    types.insert("Name".to_string(), PropertyType::Title);
    types.insert("Eng Name".to_string(), PropertyType::RichText);
    types.insert("Synopsis".to_string(), PropertyType::RichText);
    types.insert("Genre".to_string(), PropertyType::MultiSelect);
    types.insert("Cast".to_string(), PropertyType::RichText);
    types.insert("Director".to_string(), PropertyType::RichText);
    types.insert("Content Rating".to_string(), PropertyType::RichText);
    types.insert("Country of origin".to_string(), PropertyType::RichText);
    types.insert("Language".to_string(), PropertyType::RichText);
    types.insert("Release Date".to_string(), PropertyType::Date);
    types.insert("Year".to_string(), PropertyType::RichText);
    types.insert("Runtime".to_string(), PropertyType::Number);
    types.insert("Episodes".to_string(), PropertyType::Number);
    types.insert("Trailer".to_string(), PropertyType::Url);
    types.insert("IMG".to_string(), PropertyType::Url);
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
        eng_name: "TMDB Movie".to_string(),
        synopsis: Some("Movie overview".to_string()),
        genres: vec!["Drama".to_string()],
        cast: vec!["Actor A".to_string()],
        director: vec!["Director A".to_string()],
        content_rating: Some("PG-13".to_string()),
        country_of_origin: vec!["US".to_string()],
        language: Some("en".to_string()),
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
        eng_name: "TMDB Show".to_string(),
        synopsis: Some("Show overview".to_string()),
        genres: vec!["Sci-Fi".to_string()],
        cast: vec!["Actor B".to_string()],
        director: vec!["Creator B".to_string()],
        content_rating: Some("TV-MA".to_string()),
        country_of_origin: vec!["US".to_string()],
        language: Some("en".to_string()),
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
        title_property: "Name".to_string(),
        schema: Arc::new(schema),
    };

    (build_router(state), notion)
}

fn webhook_payload(updated: &[&str], page_id: &str) -> String {
    json!({
        "type": "page.properties_updated",
        "entity": { "id": page_id, "type": "page" },
        "data": {
            "updated_properties": updated
        }
    })
    .to_string()
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
    let res = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(notion.updates.lock().unwrap().is_empty());
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
    let res = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props) = &updates[0];
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
    let res = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(notion.updates.lock().unwrap().is_empty());
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
    let res = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let updates = notion.updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    let (_id, props) = &updates[0];
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
}
