# Notion database properties (required)

This file defines the exact Notion database properties CineLink expects to exist.

Notes:
- Property names are case-sensitive and must match exactly.
- CineLink updates these properties via the Notion API; missing/mismatched types will cause update failures.
- Page icon/cover are updated separately (no database property required).

## Required properties

| Property name | Notion type | Used for | Notes |
|---|---|---|---|
| `Name` | `Title` | Trigger + updated title | Title must end with `;` to trigger a refresh. CineLink replaces it with the matched TMDB title (and removes the `;`). |
| `Type` | `Select` | Movie vs TV routing | Value is treated as TV if it contains `tv` (case-insensitive). |
| `Season` | `Select` (or `Rich text`) | TV season routing | Required for TV items. Accepted formats: `Mini-series`, `Season 1`, `Season 2`, or a plain number like `1`. |
| `Eng Name` | `Rich text` | Alternate title | Populated only when CineLink decides to keep the original title as `Name` (currently: French and Spanish originals). |
| `Synopsis` | `Rich text` | TMDB overview |  |
| `Genre` | `Multi-select` | TMDB genres | Stored as a list of names. |
| `Cast` | `Rich text` | TMDB cast | Stored as a comma-separated string of names. |
| `Director` | `Rich text` | Movie director / TV created-by | Stored as a comma-separated string of names. |
| `Content Rating` | `Select` | US rating | Example values: `PG-13`, `R`, `TV-MA`. |
| `Country of origin` | `Rich text` | Country list | Stored as a comma-separated string of country names (e.g. `United States, United Kingdom`). |
| `Language` | `Select` | Language name | Stored as a full language name (not a 2-letter code). |
| `Release Date` | `Date` | Release/air date | For TV, this is the season air date. |
| `Year` | `Rich text` | Release year | Derived from `Release Date`. |
| `Runtime` | `Number` | Runtime minutes | For TV seasons, this is an average episode runtime. |
| `Episodes` | `Number` | Episode count | Populated for TV seasons only. |
| `Trailer` | `URL` | YouTube link | Selected from TMDB videos when available. |
| `IMG` | `Files` | Poster URL | Stored as a single external file URL. Also used as the page icon. |
| `IMDb Page` | `URL` | imdb.com link | Built from TMDB external ids. |
| `ID` | `Number` | TMDB id | Written on every successful match. |
