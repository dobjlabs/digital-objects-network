---
name: bitcraft-create-command
description: Define a new bitcraft command.
---

# create-command

## Output rules

- Plain text only. No markdown bold, italics, bullets, or headers in user-facing output. The SKILL.md draft you show the user in step 4 may be inside a single fenced block — that is the only allowed exception.
- No preamble. No closing summary. No commentary outside the four prompts and the final result line.
- Do not mention any other command or skill.

## What a command can contain

The `<body>` in step 4's template may include any combination of:

- Prose instructions for Claude.
- MCP tool calls (any bitcraft MCP tool — `run_action`, `list_inventory`, etc.).
- References to other bitcraft commands by name (Claude will trigger them).
- Inline scripts in any language inside fenced code blocks — `bash`, `python`, `node`, `ruby`, etc. When the command runs, Claude executes them via the Bash tool (e.g. `python -c '...'` for inline Python, or `node -e '...'` for inline Node).
- Sibling files: longer scripts the user wants saved alongside SKILL.md (e.g. `fetch_trades.py`). If the user's reply in step 3 describes a sibling file with its contents, save it as `(filename, contents)` in a list `<extra_files>` and write it in step 6. The body should reference such files by absolute path: `~/.claude/skills/bitcraft-<name>/<filename>`.
- Rich markdown intended for the user to read. The strict no-markdown output rule applies only to built-in commands; user-authored commands may format freely.

## Steps

Ask four questions, one at a time. Output each prompt on a single line, end the turn, and wait for the user's reply before asking the next.

1. Output exactly:

   `name?`

   Wait for reply. The reply is the kebab-case identifier (e.g. `mine-stone-x10`). Save it as `<name>`.

2. Output exactly:

   `description?`

   Wait for reply. Save as `<description>`.

3. Output exactly:

   `what should it do?`

   Wait for reply. Save the entire reply as `<body>`. If the reply describes sibling files with their contents (e.g. "save this as run.py" followed by code), extract each `(filename, contents)` pair into the list `<extra_files>`. Otherwise `<extra_files>` is empty.

4. Assemble the SKILL.md draft using exactly this template:

   ```
   ---
   name: bitcraft-<name>
   description: <description>
   ---

   # <name>

   <body>
   ```

   Output the draft inside a single fenced code block. Immediately below the closing fence, output two lines:

   `extra files: <comma-separated list of filenames in <extra_files>, or "(none)">`
   `confirm? (y/n)`

   End the turn. Wait for reply.

5. If the user replies `n`, return to step 3.
   If the user replies anything other than `y` or `n`, output `invalid choice` and stop.
   If the user replies `y`, continue.

6. Create the directory `~/.claude/skills/bitcraft-<name>/` if it does not exist. Write the assembled SKILL.md text from step 4 to `~/.claude/skills/bitcraft-<name>/SKILL.md`. For each `(filename, contents)` in `<extra_files>`, write to `~/.claude/skills/bitcraft-<name>/<filename>`.

7. Output exactly one line and stop:

   `command → ~/.claude/skills/bitcraft-<name>/ (reload the agent to register)`

8. On any tool error during step 6, output the error message verbatim and stop.
