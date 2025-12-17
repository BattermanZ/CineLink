# CineLink Webhook Processing (v2)

```mermaid
flowchart TD
    subgraph Request_Validation
        A[Incoming POST /] --> B{Rate limits OK?}
        B -->|no| R1[Reject 429]
        B -->|yes| C{Body size within limit?}
        C -->|no| R2[Reject 413]
        C -->|yes| D{Content-Type is application/json?}
        D -->|no| R3[Reject 415]
        D -->|yes| E{Valid Notion signature?}
        E -->|no| R4[Reject 401]
        E -->|yes| F{Valid JSON?}
        F -->|no| R5[Reject 400]
        F -->|yes| G{Event type is page.properties_updated?}
        G -->|no| Z[Ignore]
        G -->|yes| H{Fresh timestamp?}
        H -->|no| R6[Reject 400]
        H -->|yes| I{Duplicate event id?}
        I -->|yes| Z
        I -->|no| J{updated_properties contains title or season?}
        J -->|no| Z
        J -->|yes| K[Spawn async page processing]
    end

    subgraph Page_Gate
        K --> L[Fetch page properties]
        L --> M{Title ends with semicolon?}
        M -->|no| Z
        M -->|yes| N{Type indicates TV?}
        N -->|yes| O{Season present and valid?}
        O -->|no| Z
    end

    subgraph TMDB_Match
        N -->|no| P[Resolve movie id: TMDB id, IMDb tt, or search]
        O -->|yes| Q[Resolve TV id: TMDB id, IMDb tt, or search]
        P --> S[Fetch movie details]
        Q --> T[Fetch TV season details]
        S --> U[Build matched data]
        T --> U
        U -->|TMDB match/fetch fails| V[Set error title and stop]
        V --> Z
    end

    subgraph Notion_Update
        U --> W[Build Notion payload + icon/cover]
        W --> X[PATCH Notion page]
        X --> Y[Finish log]
    end
```

Key steps:
- Request validation happens before any Notion/TMDB calls: rate limiting, body size limit, content-type check, signature verification, JSON parsing, event type check, timestamp freshness, and event de-duplication.
- Webhook gating uses `updated_properties` and accepts `title` and a season update indicator (including the raw `Siv%5D` string observed from Notion).
- Title must end with `;` for processing. TV items also require a valid `Season` value.
- Type is treated as TV when the Notion `Type` select value contains `tv` (case-insensitive).
- Title text is used to resolve IDs: numeric TMDB ids or IMDb `tt...` codes bypass search; otherwise TMDB search is used.
- On TMDB match/fetch failure, the page title is set to an error message and processing stops.
- On success, Notion properties are updated and the page icon/cover are set from poster/backdrop.
