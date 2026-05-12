---
name: bitcraft-help
description: Show the list of bitcraft commands.
---

# help

## Output rules

- Plain text only. The formatter script wraps its output in a fenced code block — keep that fence in your reply; it preserves column alignment.
- No commentary outside what the script outputs.
- Do not mention any other command or skill.

## Steps

1. Run the sibling formatter script via the Bash tool:

   ```bash
   python3 "$HOME/.claude/skills/bitcraft-help/format_help.py"
   ```

2. Echo the script's stdout verbatim — byte-for-byte — as the entire reply. The script wraps output in a fenced code block; keep that fence. Do NOT:
   - Re-indent, re-align, or re-flow the text.
   - Add or remove the script's existing fence.
   - Add a header, table, bullet, or any extra markdown around the script's output.
   - Add a `bitcraft` prefix to command names.
   - Append a "run any of them by …" hint or any closing line.

3. On script error (non-zero exit, missing `python3`, etc.), output the error message verbatim, on one line. Stop.
