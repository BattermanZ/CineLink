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
        E -->|no| Z[Ignore 200]
        E -->|yes| F{Valid JSON?}
        F -->|no| R5[Reject 400]
        F -->|yes| G{Event type is page.properties_updated?}
        G -->|no| Z
        G -->|yes| I{Duplicate event id?}
        I -->|yes| Z
        I -->|no| J{updated_properties contains title or season?}
        J -->|no| Z
        J -->|yes| K[Spawn async page processing]
    end

    subgraph Page_Gate
        K --> L[Fetch page properties]
        L --> M{Title ends with ; or = ?}
        M -->|no| Z
        M -->|yes| N{Title ends with ; ?}
        N -->|yes| O{Type indicates TV?}
        O -->|yes| P{Season present and valid?}
        P -->|no| Z
        N -->|no| Q{Title ends with = ?}
    end

    subgraph TMDB_Match
        O -->|no| R[Resolve movie id: TMDB id, IMDb tt, or search]
        P -->|yes| S[Resolve TV id: TMDB id, IMDb tt, or search]
        R --> T[Fetch movie details]
        S --> U[Fetch TV season details]
        T --> V[Build matched data]
        U --> V
        V -->|TMDB match/fetch fails| W[Set error title and stop]
        W --> Z
    end

    subgraph AniList_Match
        Q -->|yes| X{Season missing?}
        X -->|yes| Y[Assume season 1]
        X -->|no| Z2[Use provided season]
        Y --> AA[Resolve anime id: AniList id or search]
        Z2 --> AA
        AA --> AB[If season >1, follow PREQUEL/SEQUEL chain]
        AB --> AC[Fetch anime details]
        AC --> AD[Build matched data]
        AD -->|AniList match/fetch fails| AE[Set error title and stop]
        AE --> Z
    end

    subgraph Notion_Update
        V --> AF[Build Notion payload + icon/cover]
        AD --> AF
        AF --> AG[PATCH Notion page]
        AG --> AH[Finish log]
    end
```

Key steps:
- Request validation happens before any Notion/TMDB/AniList calls: rate limiting, body size limit, content-type check, signature verification, JSON parsing, event type check, and event de-duplication.
- Webhook gating uses `updated_properties` and accepts `title` and a season update indicator (including the raw `Siv%5D` string observed from Notion).
- The page is considered “armed” when the title ends with `;` (TMDB) or `=` (AniList).
- TV items (TMDB flow) require a valid `Season` value. AniList uses `Season` to pick sequels but defaults to season `1` when missing.
- Type is treated as TV when the Notion `Type` select value contains `tv` (case-insensitive).
- Title text is used to resolve IDs: numeric ids or IMDb `tt...` codes bypass search; otherwise title search is used.
- On match/fetch failure, the page title is set to an error message and processing stops.
- On success, Notion properties are updated and the page icon/cover are set from poster/backdrop.
