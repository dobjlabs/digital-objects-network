---
name: bitcraft-preview
description: Open the live bitcraft dashboard in the Claude Code preview pane (falls back to the browser).
hidden: true
---

# preview

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Constants

- Server name: `bitcraft-preview`
- Port: `7719`
- Skill directory: `$HOME/.claude/skills/bitcraft-preview` (contains `index.html`)

## Steps

### 1. Ensure `.claude/launch.json` has the bitcraft-preview server entry

Resolve `$HOME` (do not leave it literal in the JSON). Compute the absolute path: `<HOME>/.claude/skills/bitcraft-preview`.

The server entry must match this shape exactly:

```json
{
  "name": "bitcraft-preview",
  "runtimeExecutable": "python3",
  "runtimeArgs": ["-m", "http.server", "7719", "--directory", "<HOME>/.claude/skills/bitcraft-preview"],
  "port": 7719
}
```

- If `.claude/launch.json` exists: parse it, locate any existing entry whose `name == "bitcraft-preview"`, and replace it with the shape above. Preserve all other entries and the surrounding object structure.
- If `.claude/launch.json` does not exist: create `.claude/` if needed, then write:
  ```json
  {
    "version": "0.0.1",
    "configurations": [<entry>]
  }
  ```

### 2. Start the preview server and open the pane

Call the MCP tool `mcp__Claude_Preview__preview_start` with `name: "bitcraft-preview"`. The preview pane opens automatically at `http://localhost:7719/` which serves `index.html`.

On success, output exactly one line and stop:

`preview pane → http://localhost:7719/`

### 3. Fallback if step 2 fails

If `mcp__Claude_Preview__preview_start` returns an error (Claude Preview MCP not installed, `python3` missing, port already bound by something else, etc.), fall back to the browser path. Do not retry the pane.

1. Detect platform via `uname -s`. Pick `open` (Darwin), `xdg-open` (Linux), or `start` (Windows/MSYS).
2. Run `<opener> "$HOME/.claude/skills/bitcraft-preview/index.html"`.
3. On success, output exactly one line and stop:

   `preview → ~/.claude/skills/bitcraft-preview/index.html (browser; pane unavailable)`

4. If the opener also fails, output exactly:

   `preview at ~/.claude/skills/bitcraft-preview/index.html (open this file manually)`

### 4. Other errors

On any tool error not handled above, output the error message verbatim, on one line. Stop.
