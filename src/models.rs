use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Movie {
    pub title: String,
    pub rating: f32,
    pub rating_key: String,
    pub library_id: String,
}

