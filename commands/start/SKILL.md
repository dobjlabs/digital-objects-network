---
name: bitcraft-start
description: Begin a bitcraft session — open the live dashboard preview pane and print the command list. Triggers on "start", "begin", "init", "open bitcraft", or any phrase meaning "start a bitcraft session".
hidden: true
---

# start

## Output rules

- The only user-facing output is the help block produced by step 5. Steps 1–4 produce NO user-facing output, NO commentary, NO acknowledgement.
- Swallow any error in steps 1–4 silently — never block step 5.
- Do not mention any other command or skill.

## Steps

### 1–4. Silently open / refresh the preview pane

1. Resolve `$HOME` to an absolute path.
2. Compute the desired `.claude/launch.json` entry:
   ```json
   {
     "name": "bitcraft-preview",
     "runtimeExecutable": "python3",
     "runtimeArgs": [
       "-m",
       "http.server",
       "7719",
       "--directory",
       "<HOME>/.claude/skills/bitcraft-preview"
     ],
     "port": 7719
   }
   ```
3. Check `.claude/launch.json` in the current working directory:
   - If it does not exist: create `.claude/` if needed and write `{"version":"0.0.1","configurations":[<entry>]}`.
   - If it exists and already contains an entry with `name == "bitcraft-preview"`: leave the file as-is.
   - If it exists but does not contain that entry: append the entry to its `configurations` array, preserving all other entries and the existing version field.
4. Force a fresh start so the pane reopens even if the user previously closed it:
   - Call `mcp__Claude_Preview__preview_list` with `{}`. If the result contains an entry with `name == "bitcraft-preview"`, call `mcp__Claude_Preview__preview_stop` with `{serverId: <that id>}`. Swallow any error.
   - Call `mcp__Claude_Preview__preview_start` with `{name: "bitcraft-preview"}`. Swallow any error.

### 5. Print the command list

Run the help formatter script via the Bash tool:

```bash
python3 "$HOME/.claude/skills/bitcraft-help/format_help.py"
```

Echo the script's stdout verbatim — byte-for-byte — as the entire reply. The script wraps output in a fenced code block; keep that fence. Do NOT modify, re-align, add a header, add a `bitcraft` prefix, or append a closing line.

On script error (non-zero exit, missing `python3`, etc.), output the error message verbatim, on one line. Stop.
