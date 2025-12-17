use anyhow::{anyhow, Result};
use std::collections::HashSet;

use super::client::{AniListClient, AniListMediaType, RelationsPayload, SearchCandidate};

impl AniListClient {
    pub async fn resolve_id(&self, media_type: AniListMediaType, query: &str) -> Result<i32> {
        if let Some(id) = parse_anilist_id(query) {
            return Ok(id);
        }
        self.search_id(media_type, query).await
    }

    pub async fn resolve_id_with_season(
        &self,
        media_type: AniListMediaType,
        query: &str,
        season: Option<i32>,
    ) -> Result<i32> {
        if let Some(id) = parse_anilist_id(query) {
            return Ok(id);
        }
        let candidate = self.pick_best_candidate(media_type, query).await?;
        let season = season.unwrap_or(1).max(1);
        self.resolve_season_entry(media_type, candidate, season)
            .await
    }

    async fn resolve_season_entry(
        &self,
        media_type: AniListMediaType,
        candidate_id: i32,
        season: i32,
    ) -> Result<i32> {
        let base = self.find_base_entry(media_type, candidate_id).await?;
        if season <= 1 {
            return Ok(base);
        }
        self.follow_sequel_chain(media_type, base, season - 1).await
    }

    async fn find_base_entry(&self, media_type: AniListMediaType, start_id: i32) -> Result<i32> {
        let mut current = start_id;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current) {
                return Ok(current);
            }
            let relations = self.fetch_relations(media_type, current).await?;
            let prequel = pick_relation_id(&relations, "PREQUEL");
            match prequel {
                Some(prev) => current = prev,
                None => return Ok(current),
            }
        }
    }

    async fn follow_sequel_chain(
        &self,
        media_type: AniListMediaType,
        start_id: i32,
        steps: i32,
    ) -> Result<i32> {
        let mut current = start_id;
        let mut seen = HashSet::new();
        for _ in 0..steps {
            if !seen.insert(current) {
                break;
            }
            let relations = self.fetch_relations(media_type, current).await?;
            let sequel = pick_best_sequel_id(&relations)
                .ok_or_else(|| anyhow!("No AniList sequel found while resolving season"))?;
            current = sequel;
        }
        Ok(current)
    }

    async fn pick_best_candidate(&self, media_type: AniListMediaType, query: &str) -> Result<i32> {
        let candidates = self.search_candidates(media_type, query).await?;
        let query_key = normalize_title_key(query);

        let mut best_id: Option<i32> = None;
        let mut best_score: i32 = i32::MIN;

        for candidate in candidates {
            let direct_score = score_candidate_title(&query_key, &candidate);
            let base_id = self
                .find_base_entry(media_type, candidate.id)
                .await
                .unwrap_or(candidate.id);
            let base_title = self.fetch_titles(media_type, base_id).await.ok();
            let base_score = base_title
                .as_ref()
                .map(|t| score_title(&query_key, t.english.as_deref(), t.romaji.as_deref()))
                .unwrap_or(0);

            let score = direct_score.saturating_add(base_score / 2);
            if score > best_score {
                best_score = score;
                best_id = Some(candidate.id);
            }
        }

        best_id.ok_or_else(|| anyhow!("No AniList match found for '{}'", query))
    }
}

fn pick_relation_id(relations: &RelationsPayload, rel: &str) -> Option<i32> {
    relations
        .edges
        .iter()
        .find(|e| e.relation_type.as_deref() == Some(rel))
        .and_then(|e| e.node.as_ref())
        .map(|n| n.id)
}

fn pick_best_sequel_id(relations: &RelationsPayload) -> Option<i32> {
    let sequels: Vec<_> = relations
        .edges
        .iter()
        .filter(|e| e.relation_type.as_deref() == Some("SEQUEL"))
        .filter_map(|e| e.node.as_ref())
        .collect();
    if sequels.is_empty() {
        return None;
    }

    let current = relations.start_date.as_ref().and_then(date_key);
    let mut best_after: Option<(i32, i32)> = None; // (key, id)
    let mut best_any: Option<(i32, i32)> = None;

    for node in sequels {
        let id = node.id;
        let key = node
            .start_date
            .as_ref()
            .and_then(date_key)
            .unwrap_or(i32::MAX);
        best_any = match best_any {
            None => Some((key, id)),
            Some((k, _)) if key < k => Some((key, id)),
            Some(v) => Some(v),
        };
        if let Some(cur) = current {
            if key > cur {
                best_after = match best_after {
                    None => Some((key, id)),
                    Some((k, _)) if key < k => Some((key, id)),
                    Some(v) => Some(v),
                };
            }
        }
    }

    best_after.or(best_any).map(|(_, id)| id)
}

fn date_key(date: &super::client::FuzzyDate) -> Option<i32> {
    let y = date.year?;
    let m = date.month.unwrap_or(1);
    let d = date.day.unwrap_or(1);
    Some(y * 10_000 + m * 100 + d)
}

fn parse_anilist_id(query: &str) -> Option<i32> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<i32>().ok().filter(|id| *id > 0)
}

fn normalize_title_key(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = false;
    for ch in input.chars() {
        let ch = ch.to_ascii_lowercase();
        let is_alnum = ch.is_ascii_alphanumeric();
        if is_alnum {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn score_candidate_title(query_key: &str, c: &SearchCandidate) -> i32 {
    score_title(query_key, c.english.as_deref(), c.romaji.as_deref())
}

fn score_title(query_key: &str, english: Option<&str>, romaji: Option<&str>) -> i32 {
    let mut best = 0;
    for (s, weight) in [(english, 100), (romaji, 90)] {
        let Some(s) = s else { continue };
        let key = normalize_title_key(s);
        if key == query_key {
            best = best.max(weight);
        } else if key.contains(query_key) || query_key.contains(&key) {
            best = best.max(weight - 20);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_anilist_id_only_for_digits() {
        assert_eq!(parse_anilist_id("176496"), Some(176496));
        assert_eq!(parse_anilist_id(" 176496 "), Some(176496));
        assert_eq!(parse_anilist_id("tt123"), None);
        assert_eq!(parse_anilist_id("abc"), None);
        assert_eq!(parse_anilist_id(""), None);
    }

    #[test]
    fn picks_best_sequel_by_start_date() {
        let relations = RelationsPayload {
            start_date: Some(super::super::client::FuzzyDate {
                year: Some(2020),
                month: Some(1),
                day: Some(1),
            }),
            edges: vec![
                super::super::client::RelationEdge {
                    relation_type: Some("SEQUEL".to_string()),
                    node: Some(super::super::client::RelationNode {
                        id: 2,
                        start_date: Some(super::super::client::FuzzyDate {
                            year: Some(2019),
                            month: Some(1),
                            day: Some(1),
                        }),
                    }),
                },
                super::super::client::RelationEdge {
                    relation_type: Some("SEQUEL".to_string()),
                    node: Some(super::super::client::RelationNode {
                        id: 3,
                        start_date: Some(super::super::client::FuzzyDate {
                            year: Some(2021),
                            month: Some(1),
                            day: Some(1),
                        }),
                    }),
                },
            ],
        };
        assert_eq!(pick_best_sequel_id(&relations), Some(3));
    }
}
