use anyhow::Result;
use std::collections::HashSet;

use super::client::{AniListClient, AniListMediaType, Media, MediaTitle, Trailer};
use super::text::clean_anilist_synopsis;
use super::AniListMapped;

impl AniListClient {
    pub(crate) async fn map_media(
        &self,
        _media_type: AniListMediaType,
        media: Media,
    ) -> Result<AniListMapped> {
        let title = media.title.unwrap_or_default();
        let (name, eng_name, original_title) = choose_titles(&title);

        let director = media
            .staff
            .and_then(|s| s.edges)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.role.as_deref().is_some_and(is_director_role))
            .filter_map(|e| e.node.and_then(|n| n.name).and_then(|n| n.full))
            .collect::<Vec<_>>();
        let director = dedupe_preserve_order(director);

        let cast = media
            .characters
            .and_then(|c| c.edges)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| e.node.and_then(|n| n.name).and_then(|n| n.full))
            .collect::<Vec<_>>();
        let cast = dedupe_preserve_order(cast);

        let country_code = media.country_of_origin.clone();
        let country_of_origin = country_code
            .as_deref()
            .map(|c| country_name_from_code(c).unwrap_or_else(|| c.to_string()));
        let language = country_code.as_deref().and_then(language_from_country);
        let is_adult = media.is_adult.unwrap_or(false);
        let content_rating = content_rating_from_is_adult(is_adult).to_string();

        let release_date = media.start_date.as_ref().and_then(fuzzy_date_to_string);
        let year = release_date
            .as_deref()
            .and_then(|d| d.split('-').next())
            .map(|s| s.to_string());

        let trailer = media.trailer.and_then(|t| trailer_url(&t));

        let poster = media
            .cover_image
            .as_ref()
            .and_then(|c| c.extra_large.clone())
            .or_else(|| media.cover_image.as_ref().and_then(|c| c.large.clone()));

        Ok(AniListMapped {
            id: media.id,
            id_mal: media.id_mal,
            name,
            eng_name,
            original_title,
            synopsis: media
                .description
                .as_deref()
                .map(clean_anilist_synopsis)
                .filter(|s| !s.trim().is_empty()),
            genres: media.genres.unwrap_or_default(),
            cast,
            director,
            is_adult,
            content_rating,
            country_of_origin,
            language,
            release_date,
            year,
            runtime_minutes: media.duration.map(|d| d as f32),
            episodes: media.episodes,
            trailer,
            poster,
            backdrop: media.banner_image,
            imdb_page: media.site_url,
        })
    }
}

fn is_director_role(role: &str) -> bool {
    let role = role.to_ascii_lowercase();
    role.contains("director") && !role.contains("assistant director")
}

fn content_rating_from_is_adult(is_adult: bool) -> &'static str {
    if is_adult {
        "Adult"
    } else {
        "All Audiences"
    }
}

fn choose_titles(title: &MediaTitle) -> (String, Option<String>, Option<String>) {
    // For anime, we avoid non-Latin scripts here:
    // - Prefer English as the "actual" title (Eng Name / main title)
    // - Use romaji as the Original Title
    // - Do not populate Eng Name (it matches the title)
    let english = title
        .english
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let romaji = title
        .romaji
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let actual = english.or(romaji).unwrap_or("Unknown Title").to_string();
    let actual = strip_trailing_season_suffix(&actual);

    let eng_name = None;
    let original_title = romaji.map(strip_trailing_season_suffix);

    (actual, eng_name, original_title)
}

pub(crate) fn strip_trailing_season_suffix(title: &str) -> String {
    let trimmed = title.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    let Some(season_idx) = lower.rfind("season") else {
        return trimmed.to_string();
    };

    // Must be a trailing "season <digits>" (optionally preceded by whitespace/punctuation).
    let after = &lower[season_idx..];
    let after = after.strip_prefix("season").unwrap_or(after);
    let after = after.trim_start();
    if after.is_empty() || !after.bytes().all(|b| b.is_ascii_digit()) {
        return trimmed.to_string();
    }

    // Ensure "season" starts at a token boundary.
    if season_idx > 0 {
        let prev = lower.as_bytes()[season_idx - 1];
        if prev.is_ascii_alphanumeric() {
            return trimmed.to_string();
        }
    }

    // Compute cut position and also remove separators like " - ", ": ", " – ".
    let mut cut = season_idx;
    while cut > 0 && trimmed.as_bytes()[cut - 1].is_ascii_whitespace() {
        cut -= 1;
    }
    while cut > 0 {
        let ch = trimmed.as_bytes()[cut - 1] as char;
        if matches!(ch, '-' | ':' | '–' | '—') {
            cut -= 1;
            while cut > 0 && trimmed.as_bytes()[cut - 1].is_ascii_whitespace() {
                cut -= 1;
            }
        } else {
            break;
        }
    }

    let stripped = trimmed[..cut].trim_end();
    if stripped.is_empty() {
        trimmed.to_string()
    } else {
        stripped.to_string()
    }
}

fn fuzzy_date_to_string(date: &super::client::FuzzyDate) -> Option<String> {
    let year = date.year?;
    let month = date.month?;
    let day = date.day?;
    Some(format!("{:04}-{:02}-{:02}", year, month, day))
}

fn trailer_url(trailer: &Trailer) -> Option<String> {
    let site = trailer.site.as_deref()?;
    let id = trailer.id.as_deref()?;
    if site.eq_ignore_ascii_case("youtube") {
        return Some(format!("https://www.youtube.com/watch?v={}", id));
    }
    if site.eq_ignore_ascii_case("dailymotion") {
        return Some(format!("https://www.dailymotion.com/video/{}", id));
    }
    None
}

fn language_from_country(country_code: &str) -> Option<String> {
    let name = match country_code {
        "JP" => "Japanese",
        "KR" => "Korean",
        "CN" | "TW" | "HK" => "Chinese",
        "US" | "GB" | "AU" | "CA" | "NZ" | "IE" => "English",
        "FR" | "BE" | "CH" => "French",
        "ES" | "MX" | "AR" | "CL" | "CO" | "PE" => "Spanish",
        "DE" | "AT" => "German",
        "IT" => "Italian",
        "PT" | "BR" => "Portuguese",
        "RU" => "Russian",
        _ => return None,
    };
    Some(name.to_string())
}

fn country_name_from_code(code: &str) -> Option<String> {
    let name = match code {
        "JP" => "Japan",
        "KR" => "South Korea",
        "CN" => "China",
        "TW" => "Taiwan",
        "HK" => "Hong Kong",
        "US" => "United States",
        "GB" => "United Kingdom",
        "FR" => "France",
        "ES" => "Spain",
        "DE" => "Germany",
        "IT" => "Italy",
        "BR" => "Brazil",
        "CA" => "Canada",
        "AU" => "Australia",
        _ => return None,
    };
    Some(name.to_string())
}

fn dedupe_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_full_fuzzy_date_only_when_complete() {
        let d = super::super::client::FuzzyDate {
            year: Some(2024),
            month: Some(1),
            day: Some(2),
        };
        assert_eq!(fuzzy_date_to_string(&d).as_deref(), Some("2024-01-02"));
        let d2 = super::super::client::FuzzyDate {
            year: Some(2024),
            month: Some(1),
            day: None,
        };
        assert_eq!(fuzzy_date_to_string(&d2), None);
    }

    #[test]
    fn title_selection_prefers_english_and_uses_romaji_as_original() {
        let t = MediaTitle {
            english: Some("English Season 2".to_string()),
            romaji: Some("Romaji Season 2".to_string()),
        };
        let (name, eng, original) = choose_titles(&t);
        assert_eq!(name, "English");
        assert_eq!(eng, None);
        assert_eq!(original.as_deref(), Some("Romaji"));
    }

    #[test]
    fn strips_trailing_season_suffix_only_at_end() {
        assert_eq!(
            strip_trailing_season_suffix("One-Punch Man Season 2"),
            "One-Punch Man"
        );
        assert_eq!(
            strip_trailing_season_suffix("One-Punch Man: Season 2"),
            "One-Punch Man"
        );
        assert_eq!(
            strip_trailing_season_suffix("One-Punch Man - Season 2"),
            "One-Punch Man"
        );
        assert_eq!(
            strip_trailing_season_suffix("Solo Leveling Season 2 - Arise"),
            "Solo Leveling Season 2 - Arise"
        );
        assert_eq!(strip_trailing_season_suffix("Season 2"), "Season 2");
    }

    #[test]
    fn director_role_matching_is_reasonable() {
        assert!(is_director_role("Director"));
        assert!(is_director_role("Series Director"));
        assert!(is_director_role("Chief Director"));
        assert!(!is_director_role("Assistant Director"));
    }

    #[test]
    fn content_rating_derived_from_is_adult() {
        assert_eq!(content_rating_from_is_adult(false), "All Audiences");
        assert_eq!(content_rating_from_is_adult(true), "Adult");
    }

    #[test]
    fn dedupes_preserving_first_occurrence() {
        let input = vec![
            "A".to_string(),
            "B".to_string(),
            "A".to_string(),
            "B".to_string(),
        ];
        assert_eq!(
            dedupe_preserve_order(input),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn dedupes_cast_like_director() {
        let input = vec![
            "Actor".to_string(),
            "Actor".to_string(),
            "Other".to_string(),
            "Actor".to_string(),
        ];
        assert_eq!(
            dedupe_preserve_order(input),
            vec!["Actor".to_string(), "Other".to_string()]
        );
    }
}
