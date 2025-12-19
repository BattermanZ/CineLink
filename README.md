# CineLink v4.1.0

CineLink is a small webhook-driven service that listens for Notion page updates and auto-populates metadata from TMDB (movies and TV seasons) and AniList (anime).

Disclaimer: This project was developed with the help of AI-assisted coding tools. Please review changes carefully before deploying.

## What it does

- Listens on port `3146` for Notion webhooks (`POST /`).
- When a page is “armed” (title ends with `;` for TMDB or `=` for AniList) and the webhook indicates a relevant property changed, CineLink:
  - Fetches the page via the Notion API.
  - Determines whether it’s a movie/TV item (TMDB flow) or an anime (AniList flow).
  - Resolves a match from the title or an ID (TMDB id / IMDb `tt...` / AniList id).
  - Fetches metadata from TMDB or AniList.
  - Updates the Notion page properties and sets:
    - page icon to the poster (miniature)
    - page cover to the backdrop (background image)
- Exposes a simple health check (`GET /health`).

The workflow is also diagrammed in `docs/workflow_v2.md`.

## How triggering works

CineLink intentionally ignores most Notion updates; it only considers a webhook “actionable” when:

- The event type is `page.properties_updated`, and
- The updated properties include either:
  - `title` (Notion’s webhook field name), or
  - the Season property update marker seen in production payloads (`Siv%5D`), or a decoded `Season`

Then, after fetching the page, it only proceeds if the page is “armed”:

- TMDB flow: title must end with `;`
  - Movies: `"<query>;"` is enough.
  - TV: title must end with `;` and a season must be present; otherwise the update is silently ignored.
- AniList flow: title must end with `=`
  - Season is optional; if missing, it defaults to season `1`.

If CineLink cannot match a title to TMDB, it updates the Notion title to an error form like:

`<original title>; | No TMDB movie match`

For AniList, the error form is similar:

`<original title>= | No AniList match`

## Supported title inputs

### TMDB (`;`)

When the title ends with `;`, the content before the suffix can be:

- A plain text title (TMDB search is used)
- A TMDB numeric id (e.g. `2316;`)
- An IMDb id (e.g. `tt22202452;`) via TMDB “Find by ID”

### AniList (`=`)

When the title ends with `=`, the content before the suffix can be:

- A plain text title (AniList search is used)
- An AniList numeric id (e.g. `176496=`)

## Notion database requirements

Your Notion database must have properties with the expected names (CineLink also tries to infer types from a fetched page if the database schema is unavailable).

The authoritative list of properties CineLink populates is in `docs/db_properties.md`.

## Configuration (.env)

Copy `.env.example` to `.env` and set:

- `NOTION_API_KEY`: Notion Internal Integration Secret (used for Notion API calls)
- `NOTION_DATABASE_ID`: target database id
- `NOTION_WEBHOOK_SECRET`: Notion webhook signing secret / verification token (used to verify `x-notion-signature`)
- `TMDB_API_KEY`: TMDB API key

## Run locally

```bash
cargo run --bin cinelink_server
```

Health check:

```bash
curl -fsS http://localhost:3146/health
```

Useful debug logging:

```bash
RUST_LOG=debug cargo run --bin cinelink_server
```

## Run with Docker

Build:

```bash
docker build -t cinelink:v4.1.0 .
```

Run:

```bash
docker run --rm -p 3146:3146 --env-file .env cinelink:v4.1.0
```

Or use `docker-compose.yml`.

Example `docker-compose.yml`:

```yaml
services:
  cinelink:
    image: cinelink:v4.1.0
    container_name: cinelink
    restart: unless-stopped
    user: "1000:1000"
    env_file:
      - .env
    ports:
      - "3146:3146"
    environment:
      RUST_LOG: info
```

Using Compose:

```bash
docker compose up -d
docker compose logs -f cinelink
```

## Security features (in-app)

- Webhook signature verification (`x-notion-signature`) using `NOTION_WEBHOOK_SECRET` (constant-time comparison).
- Invalid signatures are ignored with `200 OK` to avoid retry amplification.
- Per-IP and global rate limiting (defaults: 60/min per IP, 200/min global, small burst allowance).
- Body size limit (1MB) and strict `Content-Type: application/json`.
- Event de-duplication by webhook `id` for a short TTL.
- Limited concurrent processing (defaults to 8).

For production, still run behind a reverse proxy (TLS termination, connection-level rate limiting, and tighter network controls).

## Development

### One-off TV backfill

If you need to run a one-time “catch up” that updates all TV pages that already have a title (without `;`) and a `Season` selected, use:

```bash
cargo run --example backfill_tv -- --concurrency 8
```

Quality gates (recommended order):

```bash
cargo check
cargo test
cargo clippy -- -D warnings
cargo fmt
```

## License

AGPL-3.0-only. See `LICENSE`.
