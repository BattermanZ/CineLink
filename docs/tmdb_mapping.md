# TMDB → Notion property mapping

Based on TMDB v3 (REST) docs: https://developer.themoviedb.org/reference/getting-started  
All requests use `language=en-US` and `Authorization: Bearer $TMDB_API_KEY` (v4 token) or `api_key` query (v3 key).

## Common lookups
- Movie detail: `GET /movie/{id}?language=en-US`
- Movie credits: `GET /movie/{id}/credits`
- Movie release dates (US rating): `GET /movie/{id}/release_dates`
- Movie videos (trailers): `GET /movie/{id}/videos`
- Movie external IDs (IMDb): `GET /movie/{id}/external_ids`
- TV show detail: `GET /tv/{id}?language=en-US`
- TV content ratings (US rating): `GET /tv/{id}/content_ratings`
- TV season detail: `GET /tv/{id}/season/{season}?language=en-US`
- TV season credits: `GET /tv/{id}/season/{season}/credits`
- TV season videos (trailers): `GET /tv/{id}/season/{season}/videos`
- TV external IDs (IMDb): `GET /tv/{id}/external_ids`

## Property sourcing
- Name / Eng Name: Movie `title`; TV `name` (English-localized via `language=en-US`).
- Synopsis: Movie `overview`; TV season `overview` (fallback to show `overview`).
- Genre: `genres[].name` from movie or show detail.
- Cast: `credits.cast[].name` (take top entries, e.g., 10) from movie credits or season credits.
- Director: Movie `credits.crew` where `job == "Director"`; TV uses `show.created_by[].name`.
- Content Rating: Movies `release_dates` entry where `iso_3166_1 == "US"` → `certification`; TV `content_ratings` where `iso_3166_1 == "US"` → `rating`.
- Country of origin: `origin_country` (ISO-3166-1) from movie or show detail.
- Language: `original_language` (ISO-639-1) from detail.
- Release Date: Movie `release_date`; TV season `air_date`.
- Year: Extract year from release date above.
- Runtime: Movie `runtime` (minutes). TV: average of per-episode `runtime` in season; fallback to first `episode_run_time` from show detail.
- Episodes (TV): `season.episodes.len()`.
- Trailer: Videos endpoint (movie or season) choose YouTube where `type == "Trailer"` (fallback `Teaser`) → `https://www.youtube.com/watch?v={key}`.
- IMG (poster): `poster_path` from movie detail; TV use season `poster_path` else show `poster_path`, build `https://image.tmdb.org/t/p/original{poster_path}`.
- Backdrop: `backdrop_path` from movie detail; TV use show `backdrop_path`, build `https://image.tmdb.org/t/p/original{backdrop_path}`.
- IMDb Page: `external_ids.imdb_id` → `https://www.imdb.com/title/{imdb_id}` if present.
- ID: TMDB `id` from movie or show.
