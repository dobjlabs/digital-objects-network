# create-command

Define a new command: a named, reusable macro over the loaded plugin's actions,
stored at `~/.dobj/commands/<name>/`. Translate the user's intent into concrete
tool calls and exact output -- never echo their prose into the body.

## Output rules

- Plain text. The README draft in step 3 is the ONE allowed fenced code block;
  everything else is bare lines.
- No preamble, no commentary outside the prompts and the final result lines.
- Ask one question at a time: output the prompt on a single line, end the turn,
  wait for the reply.
- Exit handling: before parsing any reply, if it is (case-insensitive, trimmed)
  `cancel`, `quit`, `exit`, `q`, or `nevermind`, output exactly `cancelled` and
  stop -- write nothing.

## Primitives -- build the body from these

Tools the command will call (discover exact shapes by calling them):

| Tool                                                   | Use                                                                      |
| ------------------------------------------------------ | ------------------------------------------------------------------------ |
| `list_objects`                                         | the user's objects: class, fields, liveness, `fileName`                  |
| `list_actions`                                         | available actions, each a `{pluginName, name}` with input/output classes |
| `list_classes`                                         | object classes and which actions produce/consume them                    |
| `inspect_object` / `inspect_class` / `inspect_action`  | detail on one of each                                                    |
| `check_feasibility(action)`                            | whether an action can run now; missing inputs                            |
| `run_action(action, inputObjectPaths)`                 | start an action; returns a `runId`                                       |
| `get_run(runId)`                                       | poll until `status` is `succeeded`/`failed`, then read `result`          |
| `get_state_root` / `read_settings` / `get_objects_dir` | chain head / config / objects path                                       |
| `read_doc(name)`                                       | reference docs (`read_doc("list")` for the index)                        |

- Qualified action names: `run_action`'s `action` is `{pluginName, name}`. Take
  `pluginName` from `list_actions` -- do not guess it (it is the plugin's name,
  not the MCP server name).
- Running an action is async: `run_action` returns a `runId`; poll `get_run(runId)`
  until `succeeded`/`failed`, and read produced objects from the run `result`.
- Files: objects are JSON at `~/.dobj/objects/*.dobj`; this command's files live
  at `~/.dobj/commands/<name>/`.
- For body templates (interactive-picker, argument-based, multi-step planner,
  and an anti-example), read `read_doc("command-examples")`.

## Steps

1. Output exactly `name?` and wait. Save the reply as `<name>`. It must match
   `^[a-z0-9]([a-z0-9-]{0,63})$` -- lowercase letters and digits, optionally with
   hyphens, up to 64 chars, no leading hyphen. Single words like `ideas` are
   valid; hyphens are not required. Valid: `ideas`, `chop-log`, `mine-stone-x10`,
   `notes2`. Invalid: `Ideas` (uppercase), `chop_log` (underscore), `-foo`
   (leading hyphen), empty. If it does not match, output exactly
   `invalid name (lowercase letters, digits, optional hyphens; max 64 chars; no leading hyphen)`
   and stop. The names `start`, `help`, `create-command`, `consult-docs`, and
   `dashboard` are reserved; if the reply is one of those, output exactly
   `'<name>' is reserved -- pick another` and stop.

2. Output exactly `what should it do?` and wait. Save the reply as `<intent>` --
   this is intent, not the body.

3. Derive `<description>`: one concrete imperative sentence (e.g.
   `Refine one Log into a Wood.`), not the user's prose. Design the body from
   the primitives. Decide: which tools and in what order; arguments vs. an
   interactive prompt; the exact output lines; error handling; any prompts (each
   with the exit handling above); whether to chain another saved command by
   name; and whether any step is long or needs a library (if so, put it in a
   sibling script rather than prose). Output the draft as a single fenced block
   in this shape:

   ```
   ---
   name: <name>
   description: <description>
   ---

   # <name>

   ## Output rules
   - <format rules>

   ## Steps
   1. <concrete step naming a tool, path, or script>
   N. On error, output the error message verbatim. Stop.
   ```

   Below the fence, output `confirm? (y/n)` and wait.

4. If `n`, output `what to change?`, revise, and re-output the draft + `confirm? (y/n)`.
   Anything other than `y`/`n` -> output `invalid choice` and stop. If `y`, continue.

5. Call `define_command` with `{name, description, body}` (the body is everything
   after the frontmatter in the draft). For any sibling script the body
   references, write it to `~/.dobj/commands/<name>/<file>` and reference it by
   absolute path in the body.

6. Output exactly two lines and stop:

   defined: <name>
   restart the session (re-run start) to run it by name.

7. On any tool error during step 5, output the error message verbatim and stop.

## Scope -- wraps existing actions only

create-command produces commands that orchestrate actions already in the
catalog. It cannot add new object classes or recipes (that needs plugin
authoring, which has no tool here).

Before refusing an intent, VERIFY: call `list_classes` for every noun the user
named (case-insensitive, singular/plural) and `list_actions` for the verbs. Only
refuse if the target noun is absent from `list_classes` AND no action produces
it -- otherwise it is in scope, so draft a command that orchestrates the existing
actions. If it is genuinely out of scope, output exactly one line and stop:

that needs a new class or recipe, which create-command cannot add -- it only wraps existing actions.
