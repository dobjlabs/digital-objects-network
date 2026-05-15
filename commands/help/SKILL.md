---
name: bitcraft-help
description: Show the list of bitcraft commands.
---

# help

## Output rules

- Echo the script's stdout VERBATIM — byte-for-byte — as the entire reply. No characters added, no characters dropped, no characters reordered.
- Plain text only. The formatter wraps the command list in a fenced code block — keep that fence; it preserves column alignment.
- The closing ``` is the terminus. The LAST three visible characters of your reply are the closing triple-backtick, followed by a single newline. NOTHING — no character, no whitespace, no HTML tag, no comment — comes after.
- Do NOT add HTML tags anywhere in your reply. The script's output contains NO HTML. A stray `</p>`, `<br>`, `</div>`, `<!-- … -->`, etc. inside or outside the fence is a bug — refuse to emit one.
- Do NOT add extra blank lines, extra spaces, leading whitespace, indentation, or any character the script did not print. Most importantly: do not insert any character between the last command line and the closing fence — this is where hallucinated tags have been observed.
- No commentary outside what the script outputs. No preamble, no closing line, no summary, no "run any of them by …" hint.
- Do not mention any other command or skill.
- Do not add a `bitcraft` prefix to command names.

## Steps

1. Run the sibling formatter script via the Bash tool:

   ```bash
   python3 "$HOME/.claude/skills/bitcraft-help/format_help.py"
   ```

2. Echo the script's stdout verbatim — byte-for-byte — as the entire reply. The script wraps the command list in a fenced code block; keep that fence. Before sending, mentally verify: my reply ends with the closing triple-backtick + newline, contains no HTML tags anywhere, and inserts no character between the last command line and the closing fence.

3. On script error (non-zero exit, missing `python3`, etc.), output the error message verbatim, on one line. Stop.
