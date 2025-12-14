# CineLink Sync Workflow

```mermaid
flowchart TD
    subgraph Startup
        A[Launch binary / container] --> B[Load .env (current dir or /app/.env)]
        B --> C[Setup logger (stdout + logs/cinelink.log)]
        C --> D[Validate env vars (Plex, Notion, API keys, TMDB)]
        D --> E[Start Axum server on :3146]
    end

    subgraph Trigger
        F[/POST /sync or Plex webhook media.rate/] --> G{API key valid?}
        G -- no --> H[401 error]
        G -- yes --> I[Build Notion headers]
    end

    subgraph Sync
        I --> J[Fetch Plex libraries + movies]
        J --> K[Split: rated movies + all movies]
        K --> L[Plex → Notion: add new pages, fill missing ratings]
        K --> M[Notion → Plex: apply ratings by title match]
    end

    subgraph TV Shows (optional)
        N[/POST /update-tv-shows/] --> O{API key valid?}
        O -- no --> P[401 error]
        O -- yes --> Q[Query Notion pages with Type = TV Series]
        Q --> R[For each page: get TMDB show + season data]
        R --> S[Update Notion fields: synopsis, cast, trailer, year, cover/icon]
    end

    E --> F
    E --> N
    L --> T[Log results]
    M --> T
    S --> T
```

**Quick read**
- Startup loads env, sets logging, and exposes port 3146.
- `/sync` or the Plex `media.rate` webhook runs a bidirectional sync: Plex rated movies are added to Notion; Notion-rated movies overwrite Plex ratings (exact title match).
- `/update-tv-shows` enriches Notion TV Series pages with TMDB details (cast, synopsis, trailer, cover).
- All routes require API keys; failures are logged in `logs/cinelink.log` and stdout.
