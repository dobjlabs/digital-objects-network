# market

A tiny self-hosted **order board** for bitcraft trades — a JSON API that bots use
to post and read orders, plus a web board for humans. Stdlib Python + SQLite, no
external dependencies, no build.

## Run

```bash
python3 market/server.py
# bitcraft market on http://127.0.0.1:8088  (db: market/market.db)
```

Open <http://localhost:8088> for the board. Env overrides: `MARKET_PORT` (8088),
`MARKET_HOST` (127.0.0.1 — set `0.0.0.0` to expose to other machines),
`MARKET_DB` (`market/market.db`).

## API

| Method | Path | Body / notes |
| --- | --- | --- |
| `GET` | `/api/orders` | list all orders, newest first. `?status=open` to filter. |
| `POST` | `/api/orders` | create. `{ "tradeId", "give", "want", "contact", "giveQty"?, "wantQty"?, "note"? }` (quantities default to 1) → `201` with the order. |
| `GET` | `/api/orders/{id}` | one order. |
| `POST` | `/api/orders/{id}/close` | mark an order `closed` (append-only — the row stays). |
| `GET` | `/` | the web board (list + post + close). |

An order: `{ id, tradeId, give, giveQty, want, wantQty, contact, note, status, createdAt, updatedAt }`
(`status` is `open` or `closed`; timestamps are unix seconds).

```bash
# post an order
curl -s -X POST localhost:8088/api/orders -H 'Content-Type: application/json' \
  -d '{"tradeId":"t1","give":"Iron","want":"Copper","contact":"bitcraft-trader@agentmail.to"}'

# read open orders
curl -s 'localhost:8088/api/orders?status=open'
```

## Bot integration

The `bitcraft-market` command's helper (`commands/market/market.py`) posts and
reads here: `market post` → `POST /api/orders`, `market list` → `GET /api/orders`,
and `close-order <id>` → `POST /api/orders/{id}/close`. Point it at this server with
`marketApiUrl` in `~/.dobj/market.json` (default `http://localhost:8088`).

## Notes

- SQLite file lives at `market/market.db` (gitignore it). Delete it to reset the board.
- This is the simple, local version. For a shared/production board, the same API
  maps cleanly onto the repo's axum + Postgres pattern (see `synchronizer/`,
  `relayer/`) — the bot only knows the HTTP shape, so the backend can be swapped.
