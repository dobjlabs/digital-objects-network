#!/usr/bin/env python3
"""Idempotently ensure ~/.claude/settings.json has the bitcraft compact-re-injection SessionStart hook.

When Claude Code auto-compacts a conversation, regular agent messages (including
the help block printout) are summarized away. The MCP instructions and recently-
invoked skill bodies are preserved, but the *live* command list and a dispatch
reminder are not — they need to be re-injected.

This script registers a SessionStart hook (matcher="compact") whose command
re-emits the dispatch rules + the help block via format_help.py. Anything that
hook prints to stdout becomes fresh context for Claude after compaction.

The merge is non-destructive: other top-level settings, other hook events, and
unrelated SessionStart entries are preserved. If a previous bitcraft entry
already exists (identified by the MARKER substring), it is updated in place so
re-running this script picks up any tweaks to the command string.
"""

import json
import sys
from pathlib import Path

SETTINGS = Path.home() / ".claude" / "settings.json"
MARKER = "bitcraft re-injection after compact"

COMMAND = (
    "echo '— bitcraft re-injection after compact —'; "
    "echo; "
    "echo 'Dispatch reminder: user types a bitcraft command name (no prefix) → invoke bitcraft-<name>. "
    "Anything else → reply exactly: no such bitcraft command — type create-command to define one'; "
    "echo; "
    'python3 "$HOME/.claude/skills/bitcraft-help/format_help.py"'
)

HOOK_ENTRY = {
    "matcher": "compact",
    "hooks": [
        {"type": "command", "command": COMMAND},
    ],
}


def load_settings() -> dict:
    if not SETTINGS.exists():
        return {}
    try:
        data = json.loads(SETTINGS.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return {}
    return data if isinstance(data, dict) else {}


def is_bitcraft_compact_hook(entry: dict) -> bool:
    if not isinstance(entry, dict) or entry.get("matcher") != "compact":
        return False
    for h in entry.get("hooks", []) or []:
        if isinstance(h, dict) and MARKER in (h.get("command") or ""):
            return True
    return False


def main() -> int:
    settings = load_settings()

    hooks = settings.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        print("settings.json `hooks` is not an object; refusing to clobber.", file=sys.stderr)
        return 1

    sessions = hooks.setdefault("SessionStart", [])
    if not isinstance(sessions, list):
        print("settings.json `hooks.SessionStart` is not an array; refusing to clobber.", file=sys.stderr)
        return 1

    replaced = False
    for i, entry in enumerate(sessions):
        if is_bitcraft_compact_hook(entry):
            sessions[i] = HOOK_ENTRY
            replaced = True
            break
    if not replaced:
        sessions.append(HOOK_ENTRY)

    SETTINGS.parent.mkdir(parents=True, exist_ok=True)
    SETTINGS.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
