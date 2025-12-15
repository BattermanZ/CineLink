# CineLink Webhook Processing (v2)

```mermaid
flowchart TD
    subgraph Incoming
        A["Notion webhook<br/>page.properties_updated"] --> B{updated_properties<br/>contains title/season/Siv%5D?}
        B -- "no" --> Z[Ignore]
        B -- "yes" --> C[Fetch page properties]
    end

    subgraph Gate
        C --> D[Title ends with semicolon]
        D -- "no" --> Z
        D -- "yes" --> E{Type == TV?}
        E -- "yes" --> F{Season present/valid?}
        F -- "no" --> Z
    end

    subgraph TMDB_Match
        E -- "no" --> G["Resolve movie id<br/>(TMDB id or IMDb tt in title<br/>else search)"]
        F -- "yes" --> H["Resolve TV id<br/>(TMDB id or IMDb tt in title<br/>else search)"]
        G --> I[Fetch movie details]
        H --> J[Fetch TV season details]
        I --> K[Matched media]
        J --> K
        K -- "fetch fail" --> L["Set error title<br/>No TMDB match"]
        L --> Z
    end

    subgraph Update
        K --> M["Build Notion payload<br/>fields, poster->icon,<br/>backdrop->cover"]
        M --> N[PATCH Notion page]
        N --> O[Finish log]
    end
```

Key steps:
- Webhook must list an updated property matching `title`, `season`, or the raw `Siv%5D` string observed from Notion.
- Title must end with `;`. TV requires a valid Season value.
- Title text is used to resolve IDs: numeric TMDB IDs or IMDb `tt...` codes bypass search; otherwise TMDB search is used.
- On match failure, the page title is set to an error message and processing stops.
- On success, Notion is updated (fields + icon/cover) and a completion log is emitted.
