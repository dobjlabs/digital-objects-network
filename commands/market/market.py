#!/usr/bin/env python3
"""AgentMail + market helper for the bitcraft-market command.

A pure-REST (urllib) AgentMail client authenticated by ~/.dobj/agentmail.key —
no CLI, no MCP, no OAuth. Every AgentMail / config / processed-log
operation the command needs is a deterministic subcommand here, so the agent
only makes bitcraft MCP calls and trade decisions; it never improvises HTTP
calls or output parsing.

Subcommands:
  signup <human-email> <username>              POST /agent/sign-up; persist key + inbox
  verify <otp-code>                            POST /agent/verify
  status                                       READY (key+inbox present) or NEW; honors DOBJ_HOME
  sync-config                                  contactEmail := agentmailInboxId when empty
  announce <giveQty> <give> <wantQty> <want>   post a new offer; server assigns the tradeId
  list-orders                                  read all open orders from the market board
  my-offers                                    list my own open offers (matched by contact)
  close-order <id>                             close (retire) an order on the market board
  poll <tradeId>                               list unread #<tradeId> mail; download all .dobj attachments
  reply <message_id> <text> <file...>          reply with attachments; move sent files out of inventory (.sent/)
  mark-processed <tradeId> <msg_id>            record a message id as handled

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
# The driver's `.dobj` root. `DOBJ_HOME` (same env var the Rust driver honors)
# relocates it so two agents on one machine keep separate identities + state;
# unset → `~/.dobj`. Everything below (key, config, processed logs) derives
# from this, so a per-agent root isolates the whole market identity.
DOBJ = os.environ.get("DOBJ_HOME", "").strip() or os.path.join(HOME, ".dobj")
KEY_PATH = os.path.join(DOBJ, "agentmail.key")
CFG_PATH = os.path.join(DOBJ, "market.json")
DEFAULT_USERNAME = "bitcraft-trader"
DEFAULT_MARKET_URL = "http://localhost:8088"


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


def status(argv):
    """Report whether this agent's inbox is already provisioned, honoring
    DOBJ_HOME — so the Setup flow checks the *right* root, not a hardcoded
    ~/.dobj. READY (key present + inbox known) → also prints the address."""
    cfg = load_cfg()
    inbox = (cfg.get("agentmailInboxId") or "").strip()
    if read_key() and inbox:
        emit("STATUS=READY")
        emit("inbox=" + inbox)
        return 0
    emit("STATUS=NEW")
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
    return ((cfg.get("marketApiUrl") or "").strip() or DEFAULT_MARKET_URL).rstrip("/")


def announce(argv):
    """announce <giveQty> <give> <wantQty> <want> — post a NEW offer. The server
    assigns the tradeId; we echo it back (clients never choose it)."""
    if len(argv) < 4:
        emit("STATUS=USAGE")
        return 2
    try:
        give_qty, want_qty = int(argv[0]), int(argv[2])
    except ValueError:
        emit("STATUS=BADOFFER")
        return 1
    give, want = argv[1].strip(), argv[3].strip()
    if give_qty < 1 or want_qty < 1 or not give or not want:
        emit("STATUS=BADOFFER")  # positive quantities + non-empty class names
        return 1
    cfg = load_cfg()
    contact = (cfg.get("contactEmail") or "").strip()
    apiurl = _market_url(cfg)
    if not (contact and apiurl):
        emit("STATUS=INCOMPLETE")  # need contactEmail + marketApiUrl
        return 1
    status, data = http_json("POST", apiurl + "/api/orders", {
        "give": give, "giveQty": give_qty, "want": want, "wantQty": want_qty,
        "contact": contact, "note": "bitcraft trade desk",
    })
    if status not in (200, 201) or not isinstance(data, dict):
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    tid = data.get("tradeId", "")
    emit("STATUS=OK")
    emit("tradeId=%s" % tid)
    emit("posted offer #%s: %d %s -> %d %s" % (tid, give_qty, give, want_qty, want))
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
            "give": o.get("give"), "giveQty": o.get("giveQty"),
            "want": o.get("want"), "wantQty": o.get("wantQty"),
            "contact": o.get("contact"), "status": o.get("status"),
        }))
    emit("STATUS=OK")
    emit("count=%d" % len(orders))
    return 0


def my_offers(argv):
    """List MY open offers — board orders whose contact is my inbox. Each line
    carries the tradeId + terms so `check` knows what to poll and fulfill."""
    cfg = load_cfg()
    apiurl = _market_url(cfg)
    mine = (cfg.get("contactEmail") or cfg.get("agentmailInboxId") or "").strip().lower()
    if not apiurl:
        emit("STATUS=NOAPI")
        return 1
    if not mine:
        emit("STATUS=NOINBOX")
        return 1
    status, data = http_json("GET", apiurl + "/api/orders?status=open")
    if status != 200:
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    orders = [o for o in (data if isinstance(data, list) else [])
              if (o.get("contact") or "").strip().lower() == mine]
    for o in orders:
        emit("OFFER " + json.dumps({
            "tradeId": o.get("tradeId"),
            "give": o.get("give"), "giveQty": o.get("giveQty"),
            "want": o.get("want"), "wantQty": o.get("wantQty"),
        }))
    emit("STATUS=OK")
    emit("count=%d" % len(orders))
    return 0


def close_order(argv):
    if not argv:
        emit("STATUS=USAGE")
        return 2
    cfg = load_cfg()
    apiurl = _market_url(cfg)
    if not apiurl:
        emit("STATUS=NOAPI")
        return 1
    status, _ = http_json("POST", apiurl + "/api/orders/" + seg(argv[0]) + "/close")
    if status not in (200, 204):
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    emit("STATUS=OK")
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
        atts = [a for a in (m.get("attachments") or [])
                if (a.get("filename") or "").lower().endswith(".dobj")]
        if not atts:
            continue
        safe = re.sub(r"[^A-Za-z0-9._-]", "_", str(mid))
        paths = []
        for i, att in enumerate(atts):
            aid = att.get("attachment_id") or att.get("id")
            # The attachment endpoint returns METADATA with a signed `download_url`;
            # the raw bytes live behind that CDN URL, not at the API path itself.
            st, meta = api("GET", "/inboxes/%s/messages/%s/attachments/%s"
                           % (seg(inbox), seg(mid), seg(aid)))
            url = meta.get("download_url") if isinstance(meta, dict) else None
            if st != 200 or not url:
                continue
            try:  # pre-signed CDN URL — fetch with no auth header
                with urllib.request.urlopen(url, timeout=30) as r:
                    raw = r.read()
            except (urllib.error.HTTPError, urllib.error.URLError):
                continue
            path = "/tmp/market-%s-%d.dobj" % (safe, i)
            with open(path, "wb") as f:
                f.write(raw)
            paths.append(path)
        if not paths:
            continue
        sender = m.get("from") or m.get("from_address") or m.get("sender") or ""
        emit("TRADE " + json.dumps({"messageId": mid, "from": sender,
                                    "subject": subject, "attachmentPaths": paths}))
        found += 1

    emit("STATUS=" + ("OK" if found else "NONE"))
    emit("count=%d" % found)
    return 0


def reply(argv):
    if len(argv) < 3:
        emit("STATUS=USAGE")  # reply <message_id> <text> <file...>
        return 2
    msg_id, text, paths = argv[0], argv[1], argv[2:]
    cfg = load_cfg()
    inbox = (cfg.get("agentmailInboxId") or "").strip()
    if not inbox:
        emit("STATUS=NOINBOX")
        return 1
    attachments = []
    for path in paths:
        try:
            with open(path, "rb") as f:
                content = base64.b64encode(f.read()).decode()
        except OSError:
            emit("STATUS=NOFILE")
            emit("missing=" + path)
            return 1
        attachments.append({
            "content": content,
            "filename": os.path.basename(path),
            "content_type": "application/json",
        })
    body = {"text": text, "attachments": attachments}
    status, _ = api("POST", "/inboxes/%s/messages/%s/reply" % (seg(inbox), seg(msg_id)), body)
    if status not in (200, 201, 202):
        emit("STATUS=FAIL")
        emit("http=%d" % status)
        return 1
    # The attached objects are now the counterpart's — move our local copies out
    # of inventory so we no longer hold them live. The driver only scans
    # <objects>/ (skipping subdirs) + .nullified/, so a `.sent/` sibling drops
    # out of inventory while staying on disk. Best-effort: the mail already went
    # out, so a move failure must not fail the reply.
    sent_dir = os.path.join(DOBJ, "objects", ".sent")
    moved = 0
    try:
        os.makedirs(sent_dir, exist_ok=True)
        for path in paths:
            try:
                os.replace(path, os.path.join(sent_dir, os.path.basename(path)))
                moved += 1
            except OSError:
                pass
    except OSError:
        pass
    emit("STATUS=OK")
    emit("moved=%d" % moved)
    return 0


def main():
    if len(sys.argv) < 2:
        emit("STATUS=USAGE")
        return 2
    fns = {
        "signup": signup, "verify": verify, "sync-config": sync_config,
        "status": status, "announce": announce, "list-orders": list_orders,
        "my-offers": my_offers, "close-order": close_order, "poll": poll,
        "reply": reply, "mark-processed": mark_processed,
    }
    fn = fns.get(sys.argv[1])
    if not fn:
        emit("STATUS=USAGE")
        return 2
    return fn(sys.argv[2:])


if __name__ == "__main__":
    sys.exit(main())
