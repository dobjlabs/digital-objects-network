# message-board

HTTP service for zk-craft feed posts and responses.

## Features
- Postgres-backed feed persistence
- REST API for listing posts, creating posts, and creating responses
- Claim metadata storage (`live` / `nullified`) without proof verification
- Author identity from request IP (`x-forwarded-for` first value when present)

## API
Base path: `/api/v1`

- `GET /api/v1/posts`
  - Query params: `limit`, `cursor`, `q`, `liveOnly`
  - Returns: `{ items, nextCursor }`
- `POST /api/v1/posts`
  - Body: `{ title, description, claims }`
- `POST /api/v1/posts/:post_id/responses`
  - Body: `{ description, claims }`

## Env Vars
- `MESSAGE_BOARD_DATABASE_URL` (required)
- `MESSAGE_BOARD_BIND` (default: `127.0.0.1:3100`)
- `MESSAGE_BOARD_CORS_ORIGIN` (default: `*`)
- `MESSAGE_BOARD_MAX_PAGE_SIZE` (default: `50`)

## Run
```bash
MESSAGE_BOARD_DATABASE_URL=postgres://postgres:postgres@localhost:5432/zkcraft \
RUST_LOG=info \
cargo run -p message-board --release
```
