---
name: bitcraft-create-command
description: Define a new bitcraft command.
---

# create-command

## Output rules

- Plain text. The SKILL.md draft in step 3 is the ONE allowed fenced code block. Everything else is bare lines.
- No preamble. No commentary outside the three prompts and the final result line.
- Do not mention any other command or skill outside the draft body.

## Available primitives — REFERENCE FOR DRAFTING

When you draft a new command in step 3, build the body from these primitives. Reference them by name. Do NOT just paraphrase the user's intent — translate it into concrete tool calls, paths, or scripts.

### bitcraft MCP tools (invoked by the future agent running the new skill)

| Tool | Signature | Returns |
|---|---|---|
| `list_inventory` | `()` | array of `{id, className, fileName, status, txHash, fields, ...}` |
| `list_actions` | `()` | array of `{id, description, totalInputClasses, totalOutputClasses}` |
| `list_classes` | `()` | array of `{name, liveCount, producedBy, consumedBy}` |
| `inspect_object` | `(file_name)` | `{id, className, status, txHash, state, predicateSource}` |
| `inspect_class` | `(class_name)` | `{className, predicateSource, producedBy, consumedBy}` |
| `inspect_action` | `(action_id)` | `{id, description, totalInputClasses, totalOutputClasses, predicateSource}` |
| `run_action` | `(action, input_object_paths)` — `action` is `{pluginName, name}` | `{success, message, runId, outputs, consumed}` — blocks for proof gen |
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
| `name` | string | Skill name. Must be `bitcraft-<identifier>` where identifier is lowercase letters, numbers, and optional hyphens (max 64 chars). A single word like `ideas` is valid; hyphens are not required. The `bitcraft-` prefix is what our help script keys off. |
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
- **Sibling files** for longer scripts: save as `${CLAUDE_SKILL_DIR}/<filename>` and reference by that absolute path from the SKILL.md body. Step 5 will write any sibling files you declare.

### Chaining other bitcraft commands

Reference another command by name in prose (e.g. "if no Wood, first invoke `craft-wood`"). The agent triggers it.

## Steps

Ask the user two questions, ONE AT A TIME. Output each prompt on a single line, end the turn, wait for the reply before asking the next.

**Exit handling for every prompt below (steps 1, 2, and the confirm in 3).** Before parsing any user reply, check whether the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`. If yes, output exactly `cancelled` and stop the entire create-command flow — do not proceed to the next step, do not write any file.

1. Output exactly `name?` and wait. Save as `<name>`. Validate: the reply must match the regex `^[a-z0-9]([a-z0-9-]{0,63})$` — lowercase letters and digits, optionally with hyphens, up to 64 chars total. Single-word names like `ideas` or `notes` are VALID. Hyphens are NOT required. Examples of valid names: `ideas`, `chop-log`, `mine-stone-x10`, `notes2`. Examples of invalid names: `Ideas` (uppercase), `chop_log` (underscore), `-foo` (leading hyphen), empty string. If invalid, output `invalid name (lowercase letters, digits, optional hyphens; max 64 chars; no leading hyphen)` and stop.

2. Output exactly `what should it do?` and wait. Save as `<intent>`. The user's reply is INTENT, not the body. You will not paste it into the body verbatim.

3. **Design the skill body.** First, derive `<description>` — a single concrete sentence summarizing what the command does, in the style of existing help-line descriptions (e.g. `Chop a new Log.`, `Refine one Log into a Wood object.`, `Combine one Wood and one Stick into a WoodPick.`). It should read as an imperative or stative one-liner, capitalized, ending in a period. Do NOT ask the user for this — generate it from `<intent>`. It will go in the frontmatter and the help block.

   Then translate `<intent>` into concrete, executable steps using the primitives above. Before writing, decide:

   **CRITICAL — qualified action names.** Every `run_action` invocation takes an `action` field of shape `{pluginName, name}`. The `pluginName` is `"episode-1"` — hardcode that string into every `run_action` step you generate. It is NOT `"bitcraft"` (that's the MCP server name); it is NOT a guess. Example:

   ```
   Call `run_action` with `action={pluginName: "episode-1", name: "<Action>"}` and `input_object_paths=[...]`.
   ```

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

4. If `n`, ask `what to change?`, take the reply, revise the draft, and re-output (return to the draft + `confirm? (y/n)`). If anything other than `y` or `n`, output `invalid choice` and stop. If `y`, continue.

5. Create the directory `~/.claude/skills/bitcraft-<name>/`. Write the assembled SKILL.md to `~/.claude/skills/bitcraft-<name>/SKILL.md`. For each `(filename, contents)` in `<extra_files>`, write to `~/.claude/skills/bitcraft-<name>/<filename>`.

6. Output exactly two lines and stop:

   ```
   command → ~/.claude/skills/bitcraft-<name>/
   type `help` to see the command and to start using it.
   ```

7. On any tool error during step 5, output the error message verbatim and stop.

## Reference docs (read on demand during step 3)

| Doc | When to read |
|-----|-------------|
| `references/worked-examples.md` | When you need a template for the SKILL.md draft. Shows the interactive-picker pattern, the argument-based pattern, and an anti-example. Read this if you're unsure how to structure the body. |

These are sibling files in this skill's directory. Read with the `Read` tool when needed. They are NOT auto-loaded; only this `SKILL.md` is in context.

## Out of scope — pexe authoring is not supported

create-command produces **skills** (SKILL.md files that wrap existing actions and MCP tools). It does NOT produce **pexes** (the `.pexe` plugin archives that define the underlying classes and actions). Pexe authoring has no public tooling yet.

If the user's intent requires introducing a new class of object, a new craftable recipe, or any state transition that doesn't map to an action already in the catalog, do NOT proceed to draft a skill. Output exactly the following three lines and stop:

```
creating new classes or recipes is not yet supported (requires pexe authoring, which has no public tooling).
create-command can only wrap existing actions.
try a different intent, or run `help` to see what's available.
```

**Verification before rejecting (REQUIRED).** Pattern-matching on words is NOT enough — class names that sound novel (e.g. `Rocket`, `Engine`, `Circuit`, `MachineII`) may very well be in the catalog. Before emitting the rejection block, you MUST:

1. Call `list_classes` and scan the response for every noun the user mentioned. Compare class names case-insensitively, and try both the bare noun and its plural/singular form.
2. Call `list_actions` and check whether actions exist for the verbs the user mentioned (mine, farm, craft, build, refine, smelt, etc., concatenated with the class).

Only reject if BOTH:
- The user's intent introduces a NOUN (the desired output) that does NOT appear in `list_classes`, AND
- There is no existing action in `list_actions` that could plausibly produce the user's stated outcome with the user's stated inputs.

If the noun IS in `list_classes` and `list_actions` has at least one action that produces it, the intent is IN SCOPE — proceed to draft a skill that wraps those existing actions (the user is asking for an orchestration/UX layer over real recipes, not a new recipe).

Signals the intent genuinely needs a pexe (only after the verification above confirms it):

- The user names a noun that is verified absent from `list_classes` output AND no existing action produces something equivalent.
- The user describes a new recipe: "combine A + B → new C", where C is verified absent from `list_classes`.
- The user explicitly asks to "add a class", "make a new item type", "create a new recipe", "extend the crafting tree".

By contrast, these intents are IN SCOPE (proceed with normal draft flow):

- Inspecting, listing, filtering, or formatting existing objects / actions / classes.
- Wrapping an existing action with custom input-picking or output-rendering logic.
- **Multi-step recipes that walk the existing tech tree backwards** — e.g. "craft a Rocket using what's in inventory, mine/craft missing inputs end-to-end" is IN SCOPE if `Rocket` is in `list_classes` (it is, in episode-1) and `CraftRocket` is in `list_actions` (it is). This is just orchestration over real recipes; see `worked-examples.md` Pattern C for the template.
- Chaining multiple existing commands.
- Reporting on chain state (state root, transactions, nullifiers, etc.).
- Anything that calls existing MCP tools and produces text output.
