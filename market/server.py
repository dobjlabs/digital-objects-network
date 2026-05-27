#!/usr/bin/env python3
"""bitcraft market — a tiny self-hosted order board.

Stdlib only (http.server + sqlite3): a JSON API that bots use to post and read
orders, plus a web board for humans. No external deps, no build.

    python3 market/server.py        # serves http://localhost:8088

Env: MARKET_PORT (default 8088), MARKET_HOST (default 127.0.0.1), MARKET_DB
(default market/market.db).

API:
    GET  /api/orders[?status=open]   list orders, newest first
    POST /api/orders                 create {tradeId, give, want, contact, note?}
    GET  /api/orders/<id>            one order
    POST /api/orders/<id>/close      mark an order closed
    GET  /                           the web board (index.html)
"""
import json
import os
import secrets
import sqlite3
import string
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
DB_PATH = os.environ.get("MARKET_DB", os.path.join(HERE, "market.db"))
INDEX = os.path.join(HERE, "index.html")
HOST = os.environ.get("MARKET_HOST", "127.0.0.1")
PORT = int(os.environ.get("MARKET_PORT", "8088"))

REQUIRED = ("give", "want", "contact")


def _qty(v):
    """Coerce a quantity to a positive int, defaulting to 1."""
    try:
        n = int(v)
    except (TypeError, ValueError):
        return 1
    return n if n > 0 else 1


def _new_trade_id(conn):
    """Server-issued unique short token — the order's public handle and the
    email-subject tag. Clients do not choose it."""
    alphabet = string.ascii_lowercase + string.digits
    for _ in range(10):
        tid = "".join(secrets.choice(alphabet) for _ in range(6))
        if not conn.execute("SELECT 1 FROM orders WHERE trade_id=?", (tid,)).fetchone():
            return tid
    return "".join(secrets.choice(alphabet) for _ in range(10))


def db():
    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    return conn


def init_db():
    with db() as conn:
        conn.execute(
            """CREATE TABLE IF NOT EXISTS orders (
                   id          INTEGER PRIMARY KEY AUTOINCREMENT,
                   trade_id    TEXT NOT NULL,
                   give        TEXT NOT NULL,
                   give_qty    INTEGER NOT NULL DEFAULT 1,
                   want        TEXT NOT NULL,
                   want_qty    INTEGER NOT NULL DEFAULT 1,
                   contact     TEXT NOT NULL,
                   note        TEXT,
                   status      TEXT NOT NULL DEFAULT 'open',
                   created_at  INTEGER NOT NULL,
                   updated_at  INTEGER NOT NULL
               )"""
        )
        # Migrate pre-quantity DBs: add the columns if an older table exists.
        for col in ("give_qty", "want_qty"):
            try:
                conn.execute("ALTER TABLE orders ADD COLUMN %s INTEGER NOT NULL DEFAULT 1" % col)
            except sqlite3.OperationalError:
                pass  # column already present


def to_order(r):
    return {
        "id": r["id"], "tradeId": r["trade_id"],
        "give": r["give"], "giveQty": r["give_qty"],
        "want": r["want"], "wantQty": r["want_qty"],
        "contact": r["contact"], "note": r["note"], "status": r["status"],
        "createdAt": r["created_at"], "updatedAt": r["updated_at"],
    }


class Handler(BaseHTTPRequestHandler):
    server_version = "bitcraft-market/0.1"

    def _send(self, code, body=None, ctype="application/json"):
        data = b"" if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.end_headers()
        if data:
            self.wfile.write(data)

    def _read_json(self):
        n = int(self.headers.get("Content-Length", "0") or "0")
        if not n:
            return {}
        try:
            return json.loads(self.rfile.read(n))
        except ValueError:
            return None

    def do_OPTIONS(self):
        self._send(204)

    def do_GET(self):
        u = urlparse(self.path)
        path = u.path.rstrip("/") or "/"
        if path == "/":
            try:
                with open(INDEX, "rb") as f:
                    self._send(200, f.read(), "text/html; charset=utf-8")
            except OSError:
                self._send(404, {"error": "index.html missing"})
            return
        if path == "/api/orders":
            status = parse_qs(u.query).get("status", [None])[0]
            with db() as conn:
                if status:
                    rows = conn.execute(
                        "SELECT * FROM orders WHERE status=? ORDER BY id DESC", (status,)
                    ).fetchall()
                else:
                    rows = conn.execute("SELECT * FROM orders ORDER BY id DESC").fetchall()
            self._send(200, [to_order(r) for r in rows])
            return
        oid = self._order_id(path)
        if oid is not None:
            with db() as conn:
                r = conn.execute("SELECT * FROM orders WHERE id=?", (oid,)).fetchone()
            self._send(200, to_order(r)) if r else self._send(404, {"error": "not found"})
            return
        self._send(404, {"error": "not found"})

    def do_POST(self):
        path = urlparse(self.path).path.rstrip("/") or "/"
        if path == "/api/orders":
            body = self._read_json()
            if body is None:
                self._send(400, {"error": "invalid json"})
                return
            missing = [k for k in REQUIRED if not str(body.get(k, "")).strip()]
            if missing:
                self._send(400, {"error": "missing fields: " + ", ".join(missing)})
                return
            now = int(time.time())
            with db() as conn:
                tid = _new_trade_id(conn)
                cur = conn.execute(
                    "INSERT INTO orders (trade_id, give, give_qty, want, want_qty, contact, note, status, created_at, updated_at)"
                    " VALUES (?,?,?,?,?,?,?,'open',?,?)",
                    (tid, str(body["give"]).strip(), _qty(body.get("giveQty", 1)),
                     str(body["want"]).strip(), _qty(body.get("wantQty", 1)), str(body["contact"]).strip(),
                     (str(body.get("note", "")).strip() or None), now, now),
                )
                r = conn.execute("SELECT * FROM orders WHERE id=?", (cur.lastrowid,)).fetchone()
            self._send(201, to_order(r))
            return
        if path.startswith("/api/orders/") and path.endswith("/close"):
            parts = path.split("/")  # ['', 'api', 'orders', '<id>', 'close']
            try:
                oid = int(parts[3])
            except (ValueError, IndexError):
                self._send(404, {"error": "not found"})
                return
            with db() as conn:
                cur = conn.execute(
                    "UPDATE orders SET status='closed', updated_at=? WHERE id=?",
                    (int(time.time()), oid),
                )
                if cur.rowcount == 0:
                    self._send(404, {"error": "not found"})
                    return
                r = conn.execute("SELECT * FROM orders WHERE id=?", (oid,)).fetchone()
            self._send(200, to_order(r))
            return
        self._send(404, {"error": "not found"})

    @staticmethod
    def _order_id(path):
        parts = path.split("/")  # ['', 'api', 'orders', '<id>']
        if len(parts) == 4 and parts[1] == "api" and parts[2] == "orders":
            try:
                return int(parts[3])
            except ValueError:
                return None
        return None

    def log_message(self, *_args):
        pass  # quiet


def main():
    init_db()
    httpd = ThreadingHTTPServer((HOST, PORT), Handler)
    print("bitcraft market on http://%s:%d  (db: %s)" % (HOST, PORT, DB_PATH), flush=True)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        httpd.shutdown()


if __name__ == "__main__":
    main()
