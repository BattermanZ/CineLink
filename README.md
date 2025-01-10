# 🎬 CineLink v3.1.0

Synchronize your movie ratings between Plex and Notion, with added support for TV shows! 📺

## 🌟 Features

- **Movie Sync**
  - 🔄 Bidirectional sync between Plex and Notion
  - ⭐ Rating synchronization (1-10 scale)
  - 📝 Automatic movie entry creation in Notion
  - 🎯 Smart duplicate detection

- **TV Show Support**
  - 📺 Fetch TV show details from TMDB
  - 🎭 Cast information
  - 📝 Show synopsis
  - 🎬 Trailer links
  - 🖼️ Show posters
  - 📅 Air dates

## 🚀 Getting Started

### Prerequisites

- 🔑 Plex server with API access
- 📘 Notion database with specific properties
- 🎥 TMDB API key
- 🐳 Docker (optional)

### Required Notion Database Properties

#### Movies
- `Name` (Title)
- `Aurel's rating` (Select) with emoji options:
  - 🌗 (1/10)
  - 🌕 (2/10)
  - 🌕🌗 (3/10)
  - 🌕🌕 (4/10)
  - 🌕🌕🌗 (5/10)
  - 🌕🌕🌕 (6/10)
  - 🌕🌕🌕🌗 (7/10)
  - 🌕🌕🌕🌕 (8/10)
  - 🌕🌕🌕🌕🌗 (9/10)
  - 🌕🌕🌕🌕🌕 (10/10)
- `Years watched` (Multi-select)

#### TV Shows
- `Type` (Select) with option "TV Series"
- `Season` (Select) with options:
  - "Mini-series"
  - "Season 1"
  - "Season 2"
  etc.
- `Synopsis` (Rich text)
- `Cast` (Rich text)
- `Trailer` (URL)
- `Year` (Rich text)

### 🔧 Configuration

1. Copy `.env.example` to `.env`
2. Fill in your credentials:
   ```env
   NOTION_API_KEY=your_notion_api_key
   NOTION_DATABASE_ID=your_notion_database_id
   PLEX_URL=your_plex_url
   PLEX_TOKEN=your_plex_token
   API_KEY=your_api_key_for_sync
   TVSHOWS_API_KEY=your_api_key_for_tv_shows
   TMDB_API_KEY=your_tmdb_api_key
   ```

### 🐳 Docker Setup

```bash
# Build the image
docker build -t cinelink:3.1.0 --platform linux/amd64 .

### 📡 API Endpoints

1. **Movie Sync**
   ```bash
   curl -X POST http://server-ip:3146/sync \
     -H "Authorization: Bearer your_api_key"
   ```

2. **TV Show Update**
   ```bash
   curl -X POST http://server-ip:3146/update-tv-shows \
     -H "Authorization: Bearer your_tvshows_api_key"
   ```

## 📝 Logs

Logs are stored in `logs/cinelink.log`

## 🔒 Security

- All endpoints require API key authentication
- Separate API keys for movie sync and TV show updates
- Environment variables for sensitive credentials

## 🤝 Contributing

Feel free to submit issues and pull requests!

## 📄 License

MIT License - see LICENSE file for details 