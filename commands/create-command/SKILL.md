---
name: bitcraft-create-command
description: Bitcraft meta-command — help the user define a new bitcraft command (a SKILL.md file installed alongside the built-in commands). Use when the user types "create-command", "new command", "add a command", "make a bitcraft command", or asks to extend the bitcraft command set.
---

# create-command

## Output rules

- Plain text only. No markdown bold, italics, bullets, or headers in user-facing output. The SKILL.md draft you show the user in step 5 may be inside a single fenced block — that is the only allowed exception.
- No preamble. No closing summary. No commentary outside the four prompts and the final result line.
- Do not mention any other command or skill.

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

   Wait for reply. Save as `<body>`.

4. Assemble the SKILL.md draft using exactly this template:

   ```
   ---
   name: bitcraft-<name>
   description: <description>
   ---

   # <name>

   <body>
   ```

   Output the draft inside a single fenced code block. Then on the line immediately after the closing fence, output exactly:

   `confirm? (y/n)`

   End the turn. Wait for reply.

5. If the user replies `n`, return to step 3.
   If the user replies anything other than `y` or `n`, output `invalid choice` and stop.
   If the user replies `y`, continue.

6. Create the directory `~/.claude/skills/bitcraft-<name>/` if it does not exist. Write the assembled SKILL.md text from step 4 to `~/.claude/skills/bitcraft-<name>/SKILL.md`.

7. Output exactly one line and stop:

   `command → ~/.claude/skills/bitcraft-<name>/SKILL.md (reload the agent to register)`

8. On any tool error during steps 6, output the error message verbatim and stop.
