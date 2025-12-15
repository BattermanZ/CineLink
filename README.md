# 🎬 CineLink v4.0.0

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

## 🚀 Project Structure

```
CineLink/
├── src/
│   ├── main.rs        # Application entry point and module declarations
│   ├── models.rs      # Data structures for Movies and TV Shows
│   ├── notion.rs      # Notion API integration and database operations
│   ├── plex.rs        # Plex API integration and XML parsing
│   ├── server.rs      # HTTP server setup and API endpoints
│   ├── sync.rs        # Synchronization logic between Plex and Notion
│   ├── tmdb.rs        # TMDB API integration for TV show details
│   └── utils.rs       # Utility functions and logging setup
├── logs/
│   └── cinelink.log   # Application logs
├── .env               # Environment variables (create from .env.example)
├── .env.example       # Example environment variables template
├── Cargo.toml         # Rust dependencies and project metadata
├── Dockerfile         # Container configuration
├── LICENSE           # MIT License
└── README.md         # Project documentation
```

### 📑 File Descriptions

- **`main.rs`**: Entry point of the application. Sets up the server, initializes logging, and manages module imports.

- **`models.rs`**: Contains data structures for:
  - `Movie`: Represents a movie with title, rating, and identifiers
  - `TvShow`: Represents a TV show with season information
  - `TvSeason`: Detailed TV season information including cast and trailers

- **`notion.rs`**: Handles all Notion database operations:
  - Movie addition and updates
  - TV show updates
  - Rating synchronization
  - Database querying

- **`plex.rs`**: Manages Plex server interactions:
  - Movie library scanning
  - Rating retrieval and updates
  - XML response parsing

- **`server.rs`**: HTTP server implementation:
  - API endpoint definitions
  - Request handling
  - Authentication middleware
  - Error handling

- **`sync.rs`**: Core synchronization logic:
  - Bidirectional sync between Plex and Notion
  - Batch processing
  - Conflict resolution

- **`tmdb.rs`**: TMDB API integration:
  - TV show search
  - Season details retrieval
  - Cast and trailer information
  - Image URL handling

- **`utils.rs`**: Utility functions:
  - Rating conversion (numeric to emoji)
  - Logging setup
  - Environment variable validation

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

# Run the container
docker run -d \
  --name cinelink \
  -p 3146:3146 \
  --env-file .env \
  -v $(pwd)/logs:/app/logs \
  cinelink:3.1.0
```

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

Logs are available in two ways:

1. **File Logs**
   - Stored in `logs/cinelink.log`
   - Persistent across container restarts when using volume mount

2. **Docker Logs**
   - Available through Docker's logging system
   - Can be viewed with:
     ```bash
     # View logs directly
     docker logs cinelink

     # Follow logs
     docker logs -f cinelink
     ```
   - Compatible with logging platforms like [Dozzle](https://dozzle.dev/)
   - To use with Dozzle:
     ```bash
     docker run -d \
       --name dozzle \
       -p 8080:8080 \
       --volume=/var/run/docker.sock:/var/run/docker.sock \
       amir20/dozzle
     ```

## 🔒 Security

- All endpoints require API key authentication
- Separate API keys for movie sync and TV show updates
- Environment variables for sensitive credentials

## 🤝 Contributing

Feel free to submit issues and pull requests!

## 📄 License

MIT License - see LICENSE file for details 
