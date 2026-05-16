#!/usr/bin/env python3
"""Idempotently manage the bitcraft-preview-agents entry in the project-local
`.claude/launch.json`.

Adds (or refreshes) a python http.server entry that serves
~/.claude/skills/bitcraft-preview-agents/ on port 7720, so the Claude
Preview MCP can open the live agent dashboard.

Default mode: add or update the entry.
`--remove` mode: drop the entry (run from `just reset`).

In both modes, other configurations and the top-level `version` field
are preserved untouched.
"""

import argparse
import json
import sys
from pathlib import Path

LAUNCH = Path.cwd() / ".claude" / "launch.json"
ENTRY_NAME = "bitcraft-preview-agents"
HOME = str(Path.home())

ENTRY = {
    "name": ENTRY_NAME,
    "runtimeExecutable": "python3",
    "runtimeArgs": [
        "-m",
        "http.server",
        "7720",
        "--directory",
        f"{HOME}/.claude/skills/bitcraft-preview-agents",
    ],
    "port": 7720,
}


def load() -> dict:
    if not LAUNCH.exists():
        return {}
    try:
        data = json.loads(LAUNCH.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return {}
    return data if isinstance(data, dict) else {}


def save(data: dict) -> None:
    LAUNCH.parent.mkdir(parents=True, exist_ok=True)
    LAUNCH.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def add() -> int:
    data = load()
    configs = data.setdefault("configurations", [])
    if not isinstance(configs, list):
        print(
            "launch.json `configurations` is not an array; refusing to clobber",
            file=sys.stderr,
        )
        return 1
    data.setdefault("version", "0.0.1")
    # Drop any existing entry with our name, then re-add.
    configs[:] = [c for c in configs if c.get("name") != ENTRY_NAME]
    configs.append(ENTRY)
    save(data)
    return 0


def remove() -> int:
    data = load()
    configs = data.get("configurations")
    if not isinstance(configs, list):
        return 0
    configs[:] = [c for c in configs if c.get("name") != ENTRY_NAME]
    save(data)
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--remove", action="store_true")
    args = ap.parse_args()
    return remove() if args.remove else add()


if __name__ == "__main__":
    raise SystemExit(main())
