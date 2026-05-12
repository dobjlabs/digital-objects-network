---
name: bitcraft-preview
description: Open the live bitcraft dashboard in your browser.
---

# preview

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Determine the platform-appropriate opener via `uname -s`:
   - `Darwin` → `open`
   - `Linux` → `xdg-open`
   - anything else (Windows under WSL, MSYS, etc.) → `start` (run through `cmd.exe /c` if needed)

2. Run the opener with the absolute path to the dashboard HTML:

   ```bash
   <opener> "$HOME/.claude/skills/bitcraft-preview/preview.html"
   ```

3. On success (exit code 0), output exactly one line and stop:

   `preview → ~/.claude/skills/bitcraft-preview/preview.html`

4. If the opener fails (no display server, headless system, exit code non-zero), output exactly one line and stop:

   `preview at ~/.claude/skills/bitcraft-preview/preview.html (open this file manually)`

5. On any other tool error, output the error message verbatim, on one line. Stop.

## What the dashboard shows

(For your context — do not narrate this to the user.) The HTML is self-contained and polls live data from the running daemon and synchronizer:

- **universe** — current GSR, current block, tx count, nullifier count, GSR count, last processed slot. Source: synchronizer's `/v1/state/head`. Refreshes every 5s.
- **inventory** — every `.dobj` file in `~/.dobj/objects/` with class + status. Source: dobjd's `/inventory`. Refreshes every 2s.
- **action log** — live event stream from dobjd's `/events` SSE. Auto-reconnects on disconnect. Shows runId, phase (generateProof / commit), status (running / done / failed), and message.

If dobjd is stopped, the inventory and action log panels show as disconnected. If the synchronizer URL is unreachable, the universe panel shows as disconnected. The dashboard does not need to be restarted in any of these cases — it recovers automatically when the services come back.
