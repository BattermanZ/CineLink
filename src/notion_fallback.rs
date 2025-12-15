//! Fallback schema in case database fetch fails, matching expected property types.
use crate::notion::{PropertySchema, PropertyType};
use std::collections::HashMap;

pub fn fallback_schema() -> PropertySchema {
    let mut types = HashMap::new();
    types.insert("Name".to_string(), PropertyType::Title);
    types.insert("Eng Name".to_string(), PropertyType::RichText);
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
