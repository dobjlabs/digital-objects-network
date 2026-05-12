---
name: bitcraft-create-command
description: Define a new bitcraft command.
---

# create-command

## Output rules

- Plain text. The SKILL.md draft in step 4 is the ONE allowed fenced code block. Everything else is bare lines.
- No preamble. No commentary outside the four prompts and the final result line.
- Do not mention any other command or skill outside the draft body.

## Available primitives — REFERENCE FOR DRAFTING

When you draft a new command in step 4, build the body from these primitives. Reference them by name. Do NOT just paraphrase the user's intent — translate it into concrete tool calls, paths, or scripts.

### bitcraft MCP tools (invoked by the future agent running the new skill)

| Tool | Signature | Returns |
|---|---|---|
| `list_inventory` | `()` | array of `{id, className, fileName, status, txHash, fields, ...}` |
| `list_actions` | `()` | array of `{id, description, totalInputClasses, totalOutputClasses}` |
| `list_classes` | `()` | array of `{name, liveCount, producedBy, consumedBy}` |
| `inspect_object` | `(file_name)` | `{id, className, status, txHash, state, predicateSource}` |
| `inspect_class` | `(class_name)` | `{className, predicateSource, producedBy, consumedBy}` |
| `inspect_action` | `(action_id)` | `{id, description, totalInputClasses, totalOutputClasses, predicateSource}` |
| `run_action` | `(action_id, input_object_paths)` | `{success, message, runId, outputs, consumed}` — blocks for proof gen |
| `check_feasibility` | `(action_id)` | `{feasible, actionId, availableInputs, missingInputs}` |
| `get_state_root` | `()` | `{stateRoot}` |
| `read_settings` | `()` | `{synchronizerApiUrl, relayerApiUrl}` |
| `get_objects_dir` | `()` | `{path}` |
| `read_doc` | `(name)` | reference documentation |

### Filesystem paths

- `~/.dobj/objects/*.dobj` — JSON, one file per object. Contains class, state, predicate, and a `proof` field (large opaque hex — usually not for display).
- `~/.dobj/objects/nullified/` — consumed (dead) objects, moved here after a successful action.
- `~/.dobj/actions/*.pexe` — installed plugin archives (zk-craft action modules).
- `~/.dobj/settings.json` — `{ synchronizerApiUrl, relayerApiUrl }`.
- `~/.dobj/dobjd.log` — driver daemon logs.
- `~/.claude/skills/bitcraft-*/` — every installed bitcraft skill.

### Skill frontmatter fields

Every SKILL.md begins with a YAML block between `---` markers. All fields are optional except where noted. Use only the fields the command actually needs — don't add ones you won't use.

| Field | Type | Purpose |
|---|---|---|
| `name` | string | Skill name. Must be `bitcraft-<kebab-case>` (lowercase letters/numbers/hyphens, max 64 chars). The `bitcraft-` prefix is what our help script keys off. |
| `description` | string | One short sentence shown in `help`. Drives Claude's automatic skill triggering. Keep it concrete (combined with `when_to_use`, capped at 1,536 chars). |
| `when_to_use` | string | Additional trigger phrases / example requests. Appended to `description` in the skill listing. Useful when the description is short but you want to widen the matcher. |
| `argument-hint` | string | Autocomplete hint shown in the `/` menu. Examples: `[file_name]`, `[class_name] [count]`, `[issue-number]`. Square brackets are convention for placeholders. |
| `arguments` | string OR list | Named positional arguments for `$name` substitution. Space-separated string or YAML list, e.g. `arguments: file_name` or `arguments: [class, count]`. Names map to positions in order. |
| `disable-model-invocation` | bool | Set `true` to require explicit user invocation (Claude won't trigger automatically). Good for destructive or side-effectful commands. |
| `user-invocable` | bool | Set `false` to hide from the `/` menu while still letting Claude trigger it. |
| `hidden` | bool | **Our custom field.** Set `true` to hide from the `bitcraft help` block. The skill still works and Claude can still trigger it. Used for utility commands (like `start`) that show up via natural-language match but shouldn't clutter the help table. |
| `allowed-tools` | string OR list | Tools the skill can use without per-use permission prompts (e.g. `Bash(git *)`, `Read`, `Write`). |
| `model` | string | Model override for the skill's lifetime. |
| `effort` | string | Effort override (`low`/`medium`/`high`/`xhigh`/`max`). |
| `context` | string | Set to `fork` to run the skill in an isolated subagent context. |
| `agent` | string | When `context: fork`, which subagent type (`Explore`, `Plan`, `general-purpose`, etc.). |
| `paths` | string OR list | Glob patterns that limit when the skill is auto-activated. |

### String substitutions inside the skill body

| Variable | Meaning |
|---|---|
| `$ARGUMENTS` | All arguments passed when the skill was invoked, as a single string. |
| `$ARGUMENTS[N]` or `$N` | The Nth positional argument (0-based). `$0` is the first. |
| `$<name>` | Named argument from the `arguments` frontmatter list. With `arguments: [class, count]`, `$class` is the first arg and `$count` is the second. |
| `${CLAUDE_SKILL_DIR}` | Absolute path to the skill's directory. Use this in Bash commands to invoke sibling scripts portably: `python3 ${CLAUDE_SKILL_DIR}/script.py`. |
| `${CLAUDE_SESSION_ID}` | Current session ID. Useful for logging. |

Indexed arguments use shell-style quoting: `/skill "hello world" foo` makes `$0 = "hello world"`, `$1 = "foo"`.

### Scripting in the skill body

- **Inline scripts** in fenced code blocks: ` ```bash `, ` ```python `, ` ```node `, etc. The agent runs them via Bash (e.g. `python3 -c '...'`).
- **Dynamic context injection** — Claude Code supports a special inline syntax where a bang followed by a backticked shell command (the exclamation-mark-then-backtick form) gets replaced with the command's stdout before the agent sees the skill. Useful for cheap up-front data fetches like a `git diff HEAD` inlined into the prompt. See the Claude Code skills docs for the exact syntax — do NOT paste literal examples of this form into a SKILL.md body unless you intend the command to run.
- **Sibling files** for longer scripts: save as `${CLAUDE_SKILL_DIR}/<filename>` and reference by that absolute path from the SKILL.md body. Step 6 will write any sibling files you declare.

### Chaining other bitcraft commands

Reference another command by name in prose (e.g. "if no Wood, first invoke `craft-wood`"). The agent triggers it.

## Steps

Ask the user four questions, ONE AT A TIME. Output each prompt on a single line, end the turn, wait for the reply before asking the next.

**Exit handling for every prompt below (steps 1, 2, 3, and the confirm in 4).** Before parsing any user reply, check whether the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`. If yes, output exactly `cancelled` and stop the entire create-command flow — do not proceed to the next step, do not write any file.

1. Output exactly `name?` and wait. The reply is the kebab-case identifier. Save as `<name>`. Validate: lowercase letters / numbers / hyphens only, ≤ 64 chars. If invalid, output `invalid name (lowercase letters, numbers, hyphens; max 64)` and stop.

2. Output exactly `description?` and wait. Save as `<description>` (one short sentence — this is what shows in `help`).

3. Output exactly `what should it do?` and wait. Save as `<intent>`. The user's reply is INTENT, not the body. You will not paste it into the body verbatim.

4. **Design the skill body.** Translate `<intent>` into concrete, executable steps using the primitives above. Before writing, decide:

   - **Which MCP tool(s)** does this need? With what arguments? (Look at the Available primitives table.)
   - **Arguments vs. interactive prompting**: does this command take command-line arguments (e.g. `/inspect-object foo.dobj`) or prompt the user mid-flow? Argument-style is direct and scriptable; interactive is more discoverable. If you choose arguments, set `argument-hint` and (optionally) `arguments` in the frontmatter, and reference `$0`, `$1`, or `$name` in the body. If you choose interactive, the body asks the user and parses their reply.
   - **Output format**: specify the exact lines. Plain text MUD-style is the default for new commands. If the intent calls for richer output, allow markdown.
   - **Errors**: what does the command output on tool failure / empty inventory / invalid choice?
   - **Prerequisites**: should the command reference other bitcraft commands (e.g. "if no Log, invoke `chop-log`")?
   - **Hidden?**: if this is a utility command users shouldn't see in `help`, set `hidden: true`.
   - **Side effects?**: if this is destructive or you want explicit-only invocation, set `disable-model-invocation: true`.
   - **Scripts vs. tool calls**: simple transformations of MCP data fit inline. Anything > 20 lines or with libraries → sibling Python script. Reference it with `${CLAUDE_SKILL_DIR}/<file>`.
   - **Exit words at every prompt**: if the body has any `pick:` / `confirm?` / `name?` style prompt that waits for user input, the parse step MUST first check whether the reply is `cancel`, `quit`, `exit`, `q`, or `nevermind` (case-insensitive, trimmed). If so, output `cancelled` and stop. Bake this into every prompt step in the generated body.

   Then assemble the SKILL.md draft using this shape (filled in, not template-y — include ONLY the frontmatter fields the command actually needs):

   ```
   ---
   name: bitcraft-<name>
   description: <description>
   argument-hint: <[arg-placeholders]>      # if it takes arguments
   arguments: <name1 name2>                  # if you want $name substitutions
   hidden: true                              # if utility (not in help)
   disable-model-invocation: true            # if explicit-invocation only
   allowed-tools: <space-separated list>     # if it needs tool permissions
   ---

   # <name>

   ## Output rules
   - <plain text / markdown choice, format-specific rules>

   ## Steps
   1. <concrete step using a named MCP tool, path, or script>
   2. <…>
   N. On error, output the error message verbatim. Stop.
   ```

   If the user described one or more sibling files (e.g. "save this as `parse.py` with these contents"), extract them into `<extra_files>` as `(filename, contents)` pairs. Otherwise `<extra_files>` is empty.

   Output the assembled draft inside a single fenced code block. Immediately below the closing fence, output two lines:

   ```
   extra files: <comma-separated filenames in <extra_files>, or "(none)">
   confirm? (y/n)
   ```

   End the turn. Wait for the reply.

5. If `n`, ask `what to change?`, take the reply, revise the draft, and re-output (return to the draft + `confirm? (y/n)`). If anything other than `y` or `n`, output `invalid choice` and stop. If `y`, continue.

6. Create the directory `~/.claude/skills/bitcraft-<name>/`. Write the assembled SKILL.md to `~/.claude/skills/bitcraft-<name>/SKILL.md`. For each `(filename, contents)` in `<extra_files>`, write to `~/.claude/skills/bitcraft-<name>/<filename>`.

7. Output exactly one line and stop:

   `command → ~/.claude/skills/bitcraft-<name>/ (reload the agent to register)`

8. On any tool error during step 6, output the error message verbatim and stop.

## Reference docs (read on demand during step 4)

| Doc | When to read |
|-----|-------------|
| `references/worked-examples.md` | When you need a template for the SKILL.md draft. Shows the interactive-picker pattern, the argument-based pattern, and an anti-example. Read this if you're unsure how to structure the body. |

These are sibling files in this skill's directory. Read with the `Read` tool when needed. They are NOT auto-loaded; only this `SKILL.md` is in context.

## Out of scope — pexe authoring is not supported

create-command produces **skills** (SKILL.md files that wrap existing actions and MCP tools). It does NOT produce **pexes** (the `.pexe` plugin archives that define the underlying classes and actions). Pexe authoring has no public tooling yet.

If between steps 3 and 4 you realize the user's intent requires introducing a new class of object, a new craftable recipe, or any state transition that doesn't map to an action already in the catalog, do NOT proceed to draft a skill. Output exactly the following three lines and stop:

```
creating new classes or recipes is not yet supported (requires pexe authoring, which has no public tooling).
create-command can only wrap existing actions.
try a different intent, or run `help` to see what's available.
```

Signals the intent needs a pexe (any of these → reject as above):

- The user names a **new type of object** that isn't in the help list or in `list_classes` output (e.g. "rocket", "garden", "weapon", "sword", etc. when none of those are existing classes).
- The user describes a **new recipe**: "combine A + B → new C", where C is not an existing class.
- The user explicitly asks to "add a class", "make a new item type", "create a new recipe", "extend the crafting tree".

By contrast, these intents ARE in scope (proceed with normal draft flow):

- Inspecting, listing, filtering, or formatting existing objects / actions / classes.
- Wrapping an existing action with custom input-picking or output-rendering logic.
- Chaining multiple existing commands (e.g. "obtain a log then craft wood").
- Reporting on chain state (state root, transactions, nullifiers, etc.).
- Anything that calls existing MCP tools and produces text output.
