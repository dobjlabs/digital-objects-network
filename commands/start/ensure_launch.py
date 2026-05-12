#!/usr/bin/env python3
"""Idempotently manage the bitcraft-preview entry in the project-local
`.claude/launch.json` (i.e. CWD-relative).

The Claude Preview MCP only reads project-local launch.json — there is no
user-global fallback — so this script must run from the directory where the
user is using Claude Code. `bitcraft-start` invokes it on every call to cover
each new project the user enters.

Default mode: add or update the entry.
`--remove` mode: drop the entry (run from `just reset`).

In both modes, other configurations and the top-level `version` field are
preserved untouched.
"""

import argparse
import json
import sys
from pathlib import Path

LAUNCH = Path.cwd() / ".claude" / "launch.json"
ENTRY_NAME = "bitcraft-preview"
HOME = str(Path.home())

ENTRY = {
    "name": ENTRY_NAME,
    "runtimeExecutable": "python3",
    "runtimeArgs": [
        "-m",
        "http.server",
        "7719",
        "--directory",
        f"{HOME}/.claude/skills/bitcraft-preview",
    ],
    "port": 7719,
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
        print("launch.json `configurations` is not an array; refusing to clobber", file=sys.stderr)
        return 1
    data.setdefault("version", "0.0.1")
    for i, c in enumerate(configs):
        if isinstance(c, dict) and c.get("name") == ENTRY_NAME:
            configs[i] = ENTRY
            save(data)
            return 0
    configs.append(ENTRY)
    save(data)
    return 0


def remove() -> int:
    if not LAUNCH.exists():
        return 0
    data = load()
    configs = data.get("configurations")
    if not isinstance(configs, list):
        return 0
    data["configurations"] = [
        c for c in configs
        if not (isinstance(c, dict) and c.get("name") == ENTRY_NAME)
    ]
    save(data)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--remove", action="store_true",
                        help="Remove the entry instead of adding it.")
    args = parser.parse_args()
    return remove() if args.remove else add()


if __name__ == "__main__":
    sys.exit(main())
