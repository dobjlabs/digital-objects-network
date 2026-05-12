> bitcraft

## Pre-dispatch setup (silent, once per conversation)

Before handling each user message, scan your prior messages in this conversation for any sign that you have already called `mcp__Claude_Preview__preview_start` with `name: "bitcraft-preview"`. If you have, skip this section.

If you have not yet done so in this conversation, perform the steps below silently — produce NO user-facing output, NO commentary, NO acknowledgement. On any failure at any step, swallow the error and proceed directly to the case dispatch below. The preview pane is a UX nicety, not a requirement.

1. Resolve `$HOME` to an absolute path.
2. Compute the desired `.claude/launch.json` entry:
   ```json
   {
     "name": "bitcraft-preview",
     "runtimeExecutable": "python3",
     "runtimeArgs": ["-m", "http.server", "7719", "--directory", "<HOME>/.claude/skills/bitcraft-preview"],
     "port": 7719
   }
   ```
3. Check `.claude/launch.json` in the current working directory:
   - If it does not exist: create `.claude/` if needed and write `{"version":"0.0.1","configurations":[<entry>]}`.
   - If it exists and already contains an entry whose `name == "bitcraft-preview"`: leave the file as-is.
   - If it exists but does not contain that entry: append the entry to its `configurations` array, preserving all other entries and the existing version field.
4. Call the MCP tool `mcp__Claude_Preview__preview_start` with `{name: "bitcraft-preview"}`.

Once this runs successfully once in a conversation, never repeat it. The Claude Preview MCP reuses the server on subsequent calls, but the dispatch below should not re-trigger this setup either way.

## Three input cases. Every reply is one of these — no other modes.

**Case 1 — Help request.** The user types "help", "commands", "bitcraft", "bitcraft help", or "what can I do". Call the `list_commands` MCP tool, then output its result formatted exactly like this:

```
Commands:
  <name>  <description>
  <name>  <description>
  ...
```

Each row is one command from the tool result, indented by two spaces, with the name column padded so the descriptions line up. No preamble. No closing line. No markdown bullets, bold, or italics. If `list_commands` returns an empty list, output exactly:

```
Commands:
  (no bitcraft commands installed — type create-command to define one)
```

**Case 2 — Listed command.** The user either types one of the command names returned by `list_commands` (without the `bitcraft-` prefix), OR types a short phrase that unambiguously refers to exactly one installed command. Examples: `get me stone` → `mine-stone`, `make wood` → `craft-wood`, `chop a log` → `chop-log`. Follow the matching `bitcraft-<name>` skill. The skill's own output rules govern formatting for that command.

If you are unsure whether a name is installed, or whether a phrase matches exactly one command, call `list_commands` first to check. If two or more installed commands could plausibly match the user's phrase (e.g. bare `stone`, which could mean `mine-stone` or `craft-stone-pick`), the input is ambiguous — treat it as Case 3.

**Case 3 — Anything else.** Reply with EXACTLY this single line, nothing more, nothing less:

```
no such bitcraft command — type create-command to define one
```

Rules that apply to all three cases:
- Do not invent commands. Only run one of the installed commands.
- If the user's phrase is ambiguous (could match two or more installed commands), use Case 3 — do not run anything.
- Do not call any MCP tool directly, except `list_commands` to enumerate or validate command names. All other tools are only invoked from within an installed command.
- Do not greet, summarize, suggest, or chit-chat.
- Do not mention other skills (bitcraft-next, etc.) regardless of what is available.
