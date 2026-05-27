#!/usr/bin/env python3
"""Deterministic AgentMail bootstrap for the bitcraft-market command (step 0c).

Subcommands:
  signup <human-email> [username]   run `agentmail agent sign-up`, persist key + inbox
  verify <otp-code>                 run `agentmail agent verify`

Persists the API key to ~/.dobj/agentmail.key (mode 600) and writes
agentmailInboxId + contactEmail into ~/.dobj/market.json. Prints machine-readable
STATUS lines and never prints the API key. Pinned to AgentMail's response shape
({"api_key", "inbox_id", ...}) so it behaves identically every run — the agent
calls it instead of improvising output parsing.
"""
import json
import os
import subprocess
import sys

HOME = os.path.expanduser("~")
DOBJ = os.path.join(HOME, ".dobj")
KEY_PATH = os.path.join(DOBJ, "agentmail.key")
CFG_PATH = os.path.join(DOBJ, "market.json")
DEFAULT_USERNAME = "bitcraft-trader"


def emit(line):
    print(line, flush=True)


def read_key():
    try:
        with open(KEY_PATH) as f:
            return f.read().strip()
    except OSError:
        return ""


def _backfill_contact_email():
    """Set market.json contactEmail := agentmailInboxId when empty. Idempotent;
    leaves an explicit override untouched; writes only when it changes. Returns
    the resulting contactEmail (or "")."""
    try:
        with open(CFG_PATH) as f:
            cfg = json.load(f)
    except (OSError, ValueError):
        return ""
    inbox = (cfg.get("agentmailInboxId") or "").strip()
    if not (cfg.get("contactEmail") or "").strip() and inbox:
        cfg["contactEmail"] = inbox
        with open(CFG_PATH, "w") as f:
            json.dump(cfg, f, indent=2)
    return (cfg.get("contactEmail") or "").strip()


def sync_config(argv):
    """Deterministic replacement for ad-hoc config edits: keep contactEmail
    consistent with the inbox address."""
    addr = _backfill_contact_email()
    if not addr:
        emit("STATUS=NOINBOX")
        return 1
    emit("STATUS=OK")
    emit("contactEmail=" + addr)
    return 0


def signup(argv):
    if not argv or not argv[0].strip():
        emit("STATUS=USAGE")
        return 2
    email = argv[0].strip()
    username = argv[1].strip() if len(argv) > 1 and argv[1].strip() else DEFAULT_USERNAME

    # Idempotent: a saved key means we're already signed up. Still repair
    # market.json so contactEmail stays consistent with the inbox.
    if read_key():
        _backfill_contact_email()
        emit("STATUS=ALREADY")
        return 0

    proc = subprocess.run(
        ["agentmail", "agent", "sign-up",
         "--human-email", email, "--username", username],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        blob = (proc.stderr + proc.stdout).lower()
        emit("STATUS=" + ("TAKEN" if "taken" in blob or "exist" in blob else "FAIL"))
        return 1

    try:
        data = json.loads(proc.stdout)
    except ValueError:
        emit("STATUS=BADJSON")
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

    try:
        with open(CFG_PATH) as f:
            cfg = json.load(f)
    except (OSError, ValueError):
        cfg = {}
    cfg["agentmailInboxId"] = inbox_id
    cfg["contactEmail"] = inbox_id  # AgentMail inbox_id IS the email address
    with open(CFG_PATH, "w") as f:
        json.dump(cfg, f, indent=2)

    emit("STATUS=OK")
    emit("inbox=" + inbox_id)
    return 0


def verify(argv):
    if not argv or not argv[0].strip():
        emit("STATUS=USAGE")
        return 2
    key = read_key()
    if not key:
        emit("STATUS=NOKEY")
        return 1
    proc = subprocess.run(
        ["agentmail", "agent", "verify", "--otp-code", argv[0].strip()],
        capture_output=True, text=True,
        env=dict(os.environ, AGENTMAIL_API_KEY=key),
    )
    if proc.returncode != 0:
        emit("STATUS=VERIFYFAIL")
        return 1
    emit("STATUS=VERIFIED")
    return 0


def main():
    if len(sys.argv) < 2:
        emit("STATUS=USAGE")
        return 2
    sub = sys.argv[1]
    if sub == "signup":
        return signup(sys.argv[2:])
    if sub == "verify":
        return verify(sys.argv[2:])
    if sub == "sync-config":
        return sync_config(sys.argv[2:])
    emit("STATUS=USAGE")
    return 2


if __name__ == "__main__":
    sys.exit(main())
