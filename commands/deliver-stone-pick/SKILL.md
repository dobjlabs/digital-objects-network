---
name: bitcraft-deliver-stone-pick
description: Procure a StonePick via the A2A agent network (Concierge orchestrates Lumberjack + Stonemason + Craftsmith).
---

# deliver-stone-pick

Drives the bitcraft A2A multi-agent demo. The Concierge agent at
`http://127.0.0.1:9996` orchestrates Lumberjack, Stonemason, and
Craftsmith — each backed by its own dobjd — to deliver a fully
ZK-anchored StonePick.

Different from the local `bitcraft-craft-stone-pick` skill, which runs
the single `CraftStonePick` action against the user's own dobjd.
This one outsources to four peer agents.

## Prerequisites

All four dobjds must be running (ports `:7717 :7727 :7737 :7747`) and
all four A2A agents must be running (ports `:9996 :9997 :9998 :9999`):

```
agents/scripts/bootstrap_dobjds.sh    # terminal A
agents/scripts/run_all.sh             # terminal B
```

If they're not up, the script will fail in step 1 below.

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or
  headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Open the live agent dashboard so the user can watch progress as the
   demo runs. Best-effort — swallow any error silently:

   ```bash
   python3 "$HOME/.claude/skills/bitcraft-preview-agents/ensure_launch.py" >/dev/null 2>&1 || true
   ```

   Then call the MCP tool `mcp__Claude_Preview__preview_start` with
   `name: "bitcraft-preview-agents"`. If the call succeeds the preview
   pane opens at `http://localhost:7720/`. If it fails (Preview MCP not
   installed, port bound, etc.), continue to step 2 silently — the
   dashboard is nice-to-have, not required.

2. Run the skill-friendly client via the Bash tool, with a long timeout
   (1800000 ms = 30 min — ZK proof generation on cold caches can take
   5-15 minutes):

   ```bash
   cd agents && uv run scripts/skill_invoke.py
   ```

   The script streams one-line-per-chunk progress to stdout while the
   demo runs (the user is watching the dashboard in parallel), then
   ends with a single `RESULT:` line.

3. From the Bash output, find the **last line beginning with `RESULT:`**.
   Three cases:

   - `RESULT: StonePick → <filename>` — output exactly one line and stop:

     `StonePick → <filename>`

   - `RESULT: FAILED <reason>` — output exactly one line and stop:

     `failed: <reason>`

   - `RESULT: UNKNOWN state=<state>` — output exactly one line and stop:

     `unknown outcome (state=<state>) — check agents/.runtime/*/dobjd.log`

4. If the Bash command in step 2 errored (non-zero exit, no `RESULT:`
   line in the output), output exactly one line and stop:

   `failed: <verbatim first error line from the output>`
