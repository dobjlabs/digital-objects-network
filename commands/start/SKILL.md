---
name: bitcraft-start
description: Begin a bitcraft session — open the live dashboard preview pane and print the command list. Triggers on "start", "begin", "init", "open bitcraft", or any phrase meaning "start a bitcraft session".
hidden: true
---

# start

## Output rules

- The only user-facing output is the help block produced by step 3. Steps 1–2 produce NO user-facing output, NO commentary, NO acknowledgement.
- Swallow any error in steps 1–2 silently — never block step 3.
- Do not mention any other command or skill.

## Steps

### 1. Silently ensure project-local `.claude/launch.json` has the bitcraft-preview entry

The Claude Preview MCP reads project-local `.claude/launch.json` (CWD-relative) — there is no user-global fallback. Run the sibling helper script in the current working directory so this directory's `.claude/launch.json` gets the entry:

```bash
python3 "$HOME/.claude/skills/bitcraft-start/ensure_launch.py"
```

Idempotent: merges the entry into an existing launch.json or creates one. Swallow any error silently.

### 2. Silently open / refresh the preview pane

Force a fresh start so the pane reopens even if the user previously closed it:

- Call `mcp__Claude_Preview__preview_list` with `{}`. If the result contains an entry with `name == "bitcraft-preview"`, call `mcp__Claude_Preview__preview_stop` with `{serverId: <that id>}`. Swallow any error.
- Call `mcp__Claude_Preview__preview_start` with `{name: "bitcraft-preview"}`. Swallow any error.

### 3. Print the command list

Run the help formatter script via the Bash tool:

```bash
python3 "$HOME/.claude/skills/bitcraft-help/format_help.py"
```

Echo the script's stdout verbatim — byte-for-byte — as the entire reply. The script wraps output in a fenced code block; keep that fence. Do NOT modify, re-align, add a header, add a `bitcraft` prefix, or append a closing line.

On script error (non-zero exit, missing `python3`, etc.), output the error message verbatim, on one line. Stop.
