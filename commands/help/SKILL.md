---
name: bitcraft-help
description: Show the list of bitcraft commands.
---

# help

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, headers, or tables.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command or skill.

## Steps

1. Run the sibling formatter script via the Bash tool:

   ```bash
   python3 "$HOME/.claude/skills/bitcraft-help/format_help.py"
   ```

2. Echo the script's stdout verbatim — byte-for-byte — as the entire reply. The script wraps its own output in a fenced code block (```); keep that fence in your reply, as it preserves column alignment in the chat UI. Do NOT:
   - Re-indent, re-align, or re-flow the text.
   - Add a header, heading, table, bullet, or extra markdown around the script's output.
   - Add or remove the script's existing fence.
   - Add a `bitcraft` prefix to command names.
   - Append a "run any of them by …" hint or any closing line.

3. On script error (non-zero exit, missing python3, etc.), output the error message verbatim, on one line. Stop.

## Tuning the format

The exact layout — indentation, column gap, header text, empty-state line — is controlled entirely by `format_help.py` (the constants `INDENT`, `GAP`, `HEADER` at the top). Edit the script to change the look, then re-run `just install-commands`. No daemon rebuild needed.
