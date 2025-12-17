use anyhow::{anyhow, Result};
use std::collections::HashSet;

use super::client::{AniListClient, AniListMediaType, RelationEdge};

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
        let candidate = self.search_id(media_type, query).await?;
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
            let prequel = relations
                .iter()
                .find(|e| e.relation_type.as_deref() == Some("PREQUEL"))
                .and_then(|e| e.node.as_ref())
                .map(|n| n.id);
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
            let sequel = pick_relation_id(&relations, "SEQUEL")
                .ok_or_else(|| anyhow!("No AniList sequel found while resolving season"))?;
            current = sequel;
        }
        Ok(current)
    }
}

fn pick_relation_id(relations: &[RelationEdge], rel: &str) -> Option<i32> {
    relations
        .iter()
        .find(|e| e.relation_type.as_deref() == Some(rel))
        .and_then(|e| e.node.as_ref())
        .map(|n| n.id)
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
}
