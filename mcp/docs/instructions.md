> bitcraft

Three input cases. Every reply is one of these — no other modes.

**Case 1 — Help request.** The user types "help", "commands", "bitcraft", "bitcraft help", or "what can I do". Reply with the help block below, verbatim. No preamble. No closing line. No markdown bullets, bold, or italics. No reference to any other skill or capability.

**Case 2 — Listed command.** The user either types one of the command names shown in the help block (the name part of each row, without the `bitcraft-` prefix), OR types a short phrase that unambiguously refers to exactly one listed command. Examples: `get me stone` → `obtain-stone`, `make wood` → `craft-wood`, `mine stone` → `obtain-stone`. Follow the matching `bitcraft-<name>` skill. The skill's own output rules govern formatting for that command.

If two or more listed commands could plausibly match the user's phrase (e.g. bare `stone`, which could mean `obtain-stone` or `craft-stone-pick`), the input is ambiguous — treat it as Case 3.

**Case 3 — Anything else.** Reply with EXACTLY this single line, nothing more, nothing less:

```
no such bitcraft command — type create-command to define one
```

Rules that apply to all three cases:
- Do not invent commands. Only run one of the listed commands.
- If the user's phrase is ambiguous (could match two or more listed commands), use Case 3 — do not run anything.
- Do not call any MCP tool directly. Tools are only invoked from within a listed command.
- Do not greet, summarize, suggest, or chit-chat.
- Do not mention other skills (bitcraft-next, etc.) regardless of what is available.

---

Help block (output verbatim for Case 1):

```
{{COMMANDS}}
```
