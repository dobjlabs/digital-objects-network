#!/usr/bin/env python3
"""AgentMail + market helper for the bitcraft-market command.

A pure-REST (urllib) AgentMail client authenticated by ~/.dobj/agentmail.key —
no CLI, no MCP, no OAuth. Every AgentMail / config / processed-log
operation the command needs is a deterministic subcommand here, so the agent
only makes bitcraft MCP calls and trade decisions; it never improvises HTTP
calls or output parsing.

Subcommands:
  signup <human-email> <username>     POST /agent/sign-up; persist key + inbox
  verify <otp-code>                   POST /agent/verify
  sync-config                         contactEmail := agentmailInboxId when empty
  announce <tradeId>                  post the offer to the market board (once)
  list-orders                         read open orders from the market board
  poll <tradeId>                      list unread #<tradeId> mail, download .dobj attachments
  reply <message_id> <file> [text]    reply to a message with <file> attached
  mark-processed <tradeId> <msg_id>   record a message id as handled

Prints STATUS=... lines for the caller to branch on; never prints the API key.
"""
import base64
import json
import os
import re
import sys
import urllib.error
import urllib.request
from urllib.parse import quote

BASE = "https://api.agentmail.to"
HOME = os.path.expanduser("~")
DOBJ = os.path.join(HOME, ".dobj")
KEY_PATH = os.path.join(DOBJ, "agentmail.key")
CFG_PATH = os.path.join(DOBJ, "market.json")
DEFAULT_USERNAME = "bitcraft-trader"


def emit(line):
    print(line, flush=True)


def seg(value):
    """URL-encode a path segment (inbox ids are emails, so `@` must encode)."""
    return quote(str(value), safe="")


def read_key():
    try:
        with open(KEY_PATH) as f:
            return f.read().strip()
    except OSError:
        return ""


def load_cfg():
    try:
        with open(CFG_PATH) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_cfg(cfg):
    os.makedirs(DOBJ, exist_ok=True)
    with open(CFG_PATH, "w") as f:
        json.dump(cfg, f, indent=2)


def api(method, path, body=None, auth=True, raw=False):
    """Return (status_code, parsed_json_or_bytes). status 0 == network error."""
    headers = {}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    if auth:
        key = read_key()
        if not key:
            return 0, b"no api key"
        headers["Authorization"] = "Bearer " + key
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            payload = resp.read()
            status = resp.status
    except urllib.error.HTTPError as e:
        payload, status = e.read(), e.code
    except urllib.error.URLError as e:
        return 0, str(e.reason).encode()
    if raw:
        return status, payload
    try:
        return status, (json.loads(payload) if payload else {})
    except ValueError:
        return status, {}


# --- config ---

def _backfill_contact_email():
    cfg = load_cfg()
    inbox = (cfg.get("agentmailInboxId") or "").strip()
    if not (cfg.get("contactEmail") or "").strip() and inbox:
        cfg["contactEmail"] = inbox
        save_cfg(cfg)
    return (cfg.get("contactEmail") or "").strip()


def sync_config(argv):
    addr = _backfill_contact_email()
    if not addr:
        emit("STATUS=NOINBOX")
        return 1
    emit("STATUS=OK")
    emit("contactEmail=" + addr)
    return 0


# --- bootstrap ---

def signup(argv):
    if not argv or not argv[0].strip():
        emit("STATUS=USAGE")
        return 2
    email = argv[0].strip()
    username = argv[1].strip() if len(argv) > 1 and argv[1].strip() else DEFAULT_USERNAME

    if read_key():
        _backfill_contact_email()
        emit("STATUS=ALREADY")
        return 0

    status, data = api("POST", "/agent/sign-up",
                       {"human_email": email, "username": username}, auth=False)
    if status not in (200, 201):
        blob = json.dumps(data).lower() if isinstance(data, (dict, list)) else str(data).lower()
        emit("STATUS=" + ("TAKEN" if "taken" in blob or "exist" in blob else "FAIL"))
        return 1

    api_key = data.get("api_key")
    inbox_id = data.get("inbox_id")
    if not api_key or not inbox_id:
        emit("STATUS=MISSINGFIELDS")
        return 1

    os.makedirs(DOBJ, exist_ok=True)
    fd = os.open(KEY_PATH, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(api_key)
    cfg = load_cfg()
    cfg["agentmailInboxId"] = inbox_id
    cfg["contactEmail"] = inbox_id  # AgentMail inbox_id IS the email address
    save_cfg(cfg)
    emit("STATUS=OK")
    emit("inbox=" + inbox_id)
    return 0


def verify(argv):
    if not argv or not argv[0].strip():
        emit("STATUS=USAGE")
        return 2
    if not read_key():
        emit("STATUS=NOKEY")
        return 1
    status, _ = api("POST", "/agent/verify", {"otp_code": argv[0].strip()})
    if status not in (200, 201, 204):
        emit("STATUS=VERIFYFAIL")
        return 1
    emit("STATUS=VERIFIED")
    return 0


# --- market board ---

def http_json(method, url, body=None):
    """Generic JSON HTTP for the market board (a non-AgentMail host)."""
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if data else {}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else {})
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read())
        except ValueError:
            return e.code, {}
    except urllib.error.URLError:
        return 0, {}


def _market_url(cfg):
    return (cfg.get("marketApiUrl") or "").strip().rstrip("/")


def announce(argv):
    if not argv:
        emit("STATUS=USAGE")
        return 2
    trade_id = argv[0]
    marker = os.path.join(DOBJ, ".market-posted-" + trade_id)
    if os.path.exists(marker):
        emit("STATUS=POSTED")
        return 0
    cfg = load_cfg()
    give = (cfg.get("give") or "").strip()
    want = (cfg.get("want") or "").strip()
    contact = (cfg.get("contactEmail") or "").strip()
    api = _market_url(cfg)
    if not (give and want and contact and api):
        emit("STATUS=INCOMPLETE")  # need give + want + contactEmail + marketApiUrl
        return 1
    status, _ = http_json("POST", api + "/api/orders", {
        "tradeId": trade_id, "give": give, "want": want,
        "contact": contact, "note": "bitcraft trade desk",
    })
    if status not in (200, 201):
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    open(marker, "w").close()
    emit("STATUS=OK")
    emit("posted offer #" + trade_id)
    return 0


def list_orders(argv):
    cfg = load_cfg()
    api = _market_url(cfg)
    if not api:
        emit("STATUS=NOAPI")
        return 1
    status, data = http_json("GET", api + "/api/orders?status=open")
    if status != 200:
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    orders = data if isinstance(data, list) else []
    for o in orders:
        emit("ORDER " + json.dumps({
            "id": o.get("id"), "tradeId": o.get("tradeId"),
            "give": o.get("give"), "want": o.get("want"),
            "contact": o.get("contact"), "status": o.get("status"),
        }))
    emit("STATUS=OK")
    emit("count=%d" % len(orders))
    return 0


# --- inbox ---

def _processed_path(trade_id):
    return os.path.join(DOBJ, ".market-processed-" + trade_id + ".log")


def _processed_set(trade_id):
    try:
        with open(_processed_path(trade_id)) as f:
            return {line.strip() for line in f if line.strip()}
    except OSError:
        return set()


def mark_processed(argv):
    if len(argv) < 2:
        emit("STATUS=USAGE")
        return 2
    trade_id, msg_id = argv[0], argv[1]
    os.makedirs(DOBJ, exist_ok=True)
    with open(_processed_path(trade_id), "a") as f:
        f.write(msg_id + "\n")
    emit("STATUS=OK")
    return 0


def poll(argv):
    if not argv:
        emit("STATUS=USAGE")
        return 2
    trade_id = argv[0]
    cfg = load_cfg()
    inbox = (cfg.get("agentmailInboxId") or "").strip()
    if not inbox:
        emit("STATUS=NOINBOX")
        return 1
    needle = ("#" + trade_id).lower()
    done = _processed_set(trade_id)

    status, data = api("GET", "/inboxes/%s/messages?labels=unread&limit=20" % seg(inbox))
    if status != 200:
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    messages = data.get("messages", []) if isinstance(data, dict) else data
    if not isinstance(messages, list):
        messages = []

    found = 0
    for m in messages:
        mid = m.get("message_id") or m.get("id")
        if not mid or mid in done:
            continue
        subject = m.get("subject") or ""
        if needle not in subject.lower():
            continue
        att = next((a for a in (m.get("attachments") or [])
                    if (a.get("filename") or "").lower().endswith(".dobj")), None)
        if not att:
            continue
        aid = att.get("attachment_id") or att.get("id")
        st, raw = api("GET", "/inboxes/%s/messages/%s/attachments/%s"
                      % (seg(inbox), seg(mid), seg(aid)), raw=True)
        if st != 200:
            continue
        safe = re.sub(r"[^A-Za-z0-9._-]", "_", str(mid))
        path = "/tmp/market-%s.dobj" % safe
        with open(path, "wb") as f:
            f.write(raw)
        sender = m.get("from") or m.get("from_address") or m.get("sender") or ""
        emit("TRADE " + json.dumps({"messageId": mid, "from": sender,
                                    "subject": subject, "attachmentPath": path}))
        found += 1

    emit("STATUS=" + ("OK" if found else "NONE"))
    emit("count=%d" % found)
    return 0


def reply(argv):
    if len(argv) < 2:
        emit("STATUS=USAGE")
        return 2
    msg_id, path = argv[0], argv[1]
    text = argv[2] if len(argv) > 2 and argv[2].strip() else "Here is your trade item."
    cfg = load_cfg()
    inbox = (cfg.get("agentmailInboxId") or "").strip()
    if not inbox:
        emit("STATUS=NOINBOX")
        return 1
    try:
        with open(path, "rb") as f:
            content = base64.b64encode(f.read()).decode()
    except OSError:
        emit("STATUS=NOFILE")
        return 1
    body = {"text": text, "attachments": [{
        "content": content,
        "filename": os.path.basename(path),
        "content_type": "application/json",
    }]}
    status, _ = api("POST", "/inboxes/%s/messages/%s/reply" % (seg(inbox), seg(msg_id)), body)
    if status not in (200, 201, 202):
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    emit("STATUS=OK")
    return 0


def main():
    if len(sys.argv) < 2:
        emit("STATUS=USAGE")
        return 2
    fns = {
        "signup": signup, "verify": verify, "sync-config": sync_config,
        "announce": announce, "list-orders": list_orders, "poll": poll,
        "reply": reply, "mark-processed": mark_processed,
    }
    fn = fns.get(sys.argv[1])
    if not fn:
        emit("STATUS=USAGE")
        return 2
    return fn(sys.argv[2:])


if __name__ == "__main__":
    sys.exit(main())
