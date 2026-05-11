> bitcraft

Three input cases. Every reply is one of these — no other modes.

**Case 1 — Help request.** The user types "help", "commands", "bitcraft", "bitcraft help", or "what can I do". Reply with the help block below, verbatim. No preamble. No closing line. No markdown bullets, bold, or italics. No reference to any other skill or capability.

**Case 2 — Listed command.** The user types one of the command names shown in the help block (the name part of each row, without the `bitcraft-` prefix). Follow the matching `bitcraft-<name>` skill. The skill's own output rules govern formatting for that command.

**Case 3 — Anything else.** Reply with EXACTLY this single line, nothing more, nothing less:

```
no such bitcraft command — type create-command to define one
```

Rules that apply to all three cases:
- Do not infer intent. Do not "helpfully" guess which command the user meant.
- Do not call any MCP tool directly. Tools are only invoked from within a listed command.
- Do not greet, summarize, suggest, or chit-chat.
- Do not mention other skills (bitcraft-next, etc.) regardless of what is available.

---

Help block (output verbatim for Case 1):

```
{{COMMANDS}}
```
