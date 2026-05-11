---
name: bitcraft-create-command
description: Bitcraft meta-command — help the user define a new bitcraft command (a SKILL.md file installed alongside the built-in commands). Use when the user types "create-command", "new command", "add a command", "make a bitcraft command", or asks to extend the bitcraft command set.
---

# create-command

Guide the user through writing a new bitcraft command and installing it.

A bitcraft command is a SKILL.md file with frontmatter (`name`, `description`) plus a body that tells the agent what to do. The body can include:
- prompt text
- MCP tool calls (any bitcraft MCP tool — `run_action`, `list_inventory`, `inspect_*`, etc.)
- references to other bitcraft commands (just name them; the agent will trigger them)
- regular code blocks the agent runs (Bash, etc.)

## Steps

Ask the user these four questions, one at a time, one line each. Wait for each answer before asking the next:

1. `name?` — kebab-case identifier, e.g. `mine-stone-x10`. Will be installed as `bitcraft-<name>`.
2. `description?` — one sentence. Used by the agent to decide when to trigger this command.
3. `what should it do?` — short body describing the steps. Plain prose is fine; the user can name MCP tools, other commands, or paste code.
4. `confirm?` — show the user the assembled SKILL.md (frontmatter + body) and ask `y/n`.

If the user answers `n`, return to step 3.

If the user answers `y`:

1. Write the file to `~/.claude/skills/bitcraft-<name>/SKILL.md` using the Write tool. Create the directory first.
2. Report exactly one line:

   ```
   command → ~/.claude/skills/bitcraft-<name>/SKILL.md (reload the agent to register)
   ```

## SKILL.md template

Use this exact frontmatter shape — the agent skill loader requires it:

```
---
name: bitcraft-<name>
description: <user's answer to question 2>
---

# <name>

<user's answer to question 3>
```

No commentary. Errors print verbatim and stop.
