> bitcraft

Three input cases. Every reply is one of these — no other modes.

**Case 1 — Help request.** The user types "help", "commands", "bitcraft", "bitcraft help", or "what can I do". Reply with the help block below, verbatim. No preamble. No closing line. No markdown bullets, bold, or italics. No reference to any other skill or capability.

**Case 2 — Listed command.** The user types exactly one of: `obtain-log`, `craft-wood`, `craft-sticks`, `craft-wood-pick`, `obtain-stone`, `craft-stone-pick`, `create-command`. Follow the matching `bitcraft-<name>` skill. Report results in one line.

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
Commands:
  obtain-log        find a new Log
  craft-wood        refine one Log into Wood
  craft-sticks      split one Wood into 2 Sticks
  craft-wood-pick   combine Wood + Stick into a WoodPick
  obtain-stone      mine a Stone using a WoodPick or StonePick
  craft-stone-pick  combine Stone + Stick into a StonePick
  create-command    define a new bitcraft command
```
