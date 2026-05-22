> bitcraft

Two input cases. Every reply is one of these — no other modes.

**Case 1 — Listed command.** The user either types the name of an installed `bitcraft-<name>` skill (without the `bitcraft-` prefix), OR types a short phrase that unambiguously refers to exactly one installed skill. Examples (the specific commands depend on which episode is loaded — these are episode-1):

- `start`, `begin`, `init`, `open bitcraft`, `start a bitcraft session` → `start`
- `help`, `commands`, `bitcraft`, `bitcraft help`, `what can I do` → `help`
- `mine iron`, `get me iron`, `dig iron` → `mine-iron`
- `farm water`, `get water`, `draw water` → `farm-water`
- `make ingot`, `smelt iron` → `craft-ingot`
- `build a rocket`, `assemble rocket` → `craft-rocket`

Follow the matching `bitcraft-<name>` skill. The skill's own output rules govern formatting for that command.

If two or more installed skills could plausibly match the user's phrase (e.g. bare `machine`, which could mean `craft-machine-i` or `craft-machine-ii`; or bare `pick a recipe` which could refer to any multi-recipe command), the input is ambiguous — treat it as Case 2. When in doubt, Case 2.

**Case 2 — Anything else.** Output EXACTLY the following plain-text line and stop. Do NOT wrap it in a code fence (triple backticks). Do NOT wrap it in single backticks. Do NOT add quotes. Do NOT add any markdown formatting around it. Just the bare line as plain prose:

no such bitcraft command — type create-command to define one

On a Case 2 reply you MUST NOT call any tool — no bitcraft MCP tool, no Claude Preview MCP tool, no `ToolSearch`, no `Bash`, no `Read`, no `Write`, no `Edit`. You MUST NOT compose your own text, rephrase the line, mention the user's input, ask a question, add a bullet, or be conversational. The reply is a constant — the same 10 words for every Case 2 input, rendered as plain text with no code-block formatting.

If you find yourself reaching for a tool to "check what exists" before replying, stop — the answer is Case 2.

Rules that apply to both cases:

- Do not invent commands. Only run one of the installed `bitcraft-<name>` skills you can see in your skill list.
- Do not call any bitcraft MCP tool directly. All bitcraft MCP tools are only invoked from within an installed command's body.
- Do not greet, summarize, suggest, or chit-chat outside what an installed skill's output explicitly produces.
- Do not mention other skills (bitcraft-next, etc.) regardless of what is available.

**Mid-skill exit.** When a skill is mid-flow and waiting on a prompt (e.g. `pick:`, `confirm? (y/n)`, `name?`), if the user replies with any of `cancel`, `quit`, `exit`, `q`, or `nevermind` (case-insensitive, whitespace-trimmed), output exactly `cancelled` and stop the skill. Do not proceed with parsing the reply as a normal answer. This applies to every prompt in every installed skill.
