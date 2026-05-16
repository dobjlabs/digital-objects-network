---
name: bitcraft-preview-agents
description: Open the live A2A agent dashboard in the Claude Code preview pane.
---

# preview-agents

Opens a live HTML dashboard that subscribes to all four bitcraft dobjds'
`/events` SSE streams in parallel and renders each agent's action
progress in real time. Useful to watch while a `deliver-stone-pick`
demo runs.

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or
  headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Constants

- Server name: `bitcraft-preview-agents`
- Port: `7720`
- Skill directory: `$HOME/.claude/skills/bitcraft-preview-agents` (contains `index.html`)

## Steps

### 1. Ensure `.claude/launch.json` has the bitcraft-preview-agents entry

Run the helper to idempotently merge the entry into the project-local
`.claude/launch.json` (CWD):

```bash
python3 "$HOME/.claude/skills/bitcraft-preview-agents/ensure_launch.py"
```

Swallow any error silently.

### 2. Start the preview server and open the pane

Call the MCP tool `mcp__Claude_Preview__preview_start` with `name: "bitcraft-preview-agents"`. The preview pane opens at `http://localhost:7720/` which serves `index.html`.

On success, output exactly one line and stop:

`preview pane → http://localhost:7720/`

### 3. Fallback if step 2 fails

If `mcp__Claude_Preview__preview_start` returns an error (Claude Preview MCP not installed, `python3` missing, port already bound, etc.), fall back to the browser path. Do not retry the pane.

1. Detect platform via `uname -s`. Pick `open` (Darwin), `xdg-open` (Linux), or `start` (Windows/MSYS).
2. Run `<opener> "$HOME/.claude/skills/bitcraft-preview-agents/index.html"`.
3. On success, output exactly one line and stop:

   `preview → ~/.claude/skills/bitcraft-preview-agents/index.html (browser; pane unavailable)`

4. If the opener also fails, output exactly:

   `preview at ~/.claude/skills/bitcraft-preview-agents/index.html (open this file manually)`

### 4. Other errors

On any tool error not handled above, output the error message verbatim, on one line. Stop.
