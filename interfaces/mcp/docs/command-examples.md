# Command body templates

Templates for the README body you draft in `create-command`, plus an
anti-example. Action names below are from the bundled `craft-basics` plugin; get
the real `pluginName` and action names for the loaded plugin from `list_actions`.

## Pattern A -- interactive picker (no argument)

Good when the user does not know the exact object up front: list, then ask.

    ---
    name: show-object
    description: Show one object's fields, omitting the proof.
    ---

    # show-object

    ## Output rules
    - Plain text, one `<key>: <value>` per line. No markdown.
    - Truncate hex over 40 chars to `<first 8>..<last 6>`. Skip any field named
      like a proof, or whose value is longer than 200 chars.

    ## Steps
    1. Call `list_objects`. If empty, output `no objects` and stop.
    2. Print each as `<n>) <fileName>` (n from 1), then `pick:` on its own line. Wait.
    3. Parse the reply as an integer; if not a listed n, output `invalid choice` and stop.
    4. Call `inspect_object` with that object's `fileName`. Output, each on its
       own line, omitting any that are missing:
       - `class: <pluginName>::<name>`
       - `status: <status>`
       - `id: <contentHash>`  (truncated)
       - `tx: <txHash>`  (truncated)
       - one line per `fields` entry as `<key>: <value>`  (truncate hex; skip proof-like or >200-char values)
    5. On tool error, output the error verbatim. Stop.

## Pattern B -- argument-based

When the command is invoked with an argument (the dispatcher passes it as a
trailing "Arguments: ..." line), read it instead of prompting.

    ---
    name: show-object
    description: Show one object's fields, omitting the proof.
    ---

    # show-object

    ## Output rules
    - (same as Pattern A)

    ## Steps
    1. If no argument was passed, output `usage: show-object <fileName>` and stop.
    2. Call `inspect_object` with the argument as `fileName`. Output its fields
       as in Pattern A. On tool error, output the error verbatim. Stop.

## Pattern C -- multi-step planner (reach a target)

For a command whose job is to reach a target class: walk the recipe backwards,
reuse what is already live, and run only the missing actions. Running an action
is async -- `run_action` returns a `runId`; poll `get_run(runId)` until
`succeeded`, then read produced object paths from its `result`.

Two rules that keep it fast and correct:

- When a class has several producing actions, pick the simplest -- fewest
  inputs, no optional gates or variants.
- Track produced object paths from each run's `result`; do NOT re-call
  `list_objects` between steps (it is slower and races with the chain you are
  building).

  ***

  name: make-woodpick
  description: Make a WoodPick -- reuse inventory, find/craft missing inputs end to end.

  ***

  # make-woodpick

  ## Output rules
  - Plain text. One plan line per class, one execution line per run
    (`<Action> -> <output>`). No markdown.

  ## Recipe chain (from `list_actions`; pluginName is `craft-basics` here)

  | Target   | action        | Inputs           | Outputs    |
  | -------- | ------------- | ---------------- | ---------- |
  | Log      | FindLog       | (none)           | 1 Log      |
  | Wood     | CraftWood     | 1 Log            | 1 Wood     |
  | Stick    | CraftSticks   | 1 Wood           | 2 Stick    |
  | WoodPick | CraftWoodPick | 1 Wood + 1 Stick | 1 WoodPick |

  ## Steps
  1. Call `list_objects`; collect the live paths for Wood, Stick, and Log.
  2. Plan backwards: 1 WoodPick needs 1 live Wood + 1 live Stick; a Stick comes
     from CraftSticks (1 Wood -> 2 Stick); a Wood from 1 Log; a Log from
     FindLog. Count what is live and compute how many of each to make. Print
     one `<Class> have:<N> need:<M>` line per class.
  3. Run `FindLog` for each Log needed; after each, poll `get_run` and append
     the new Log path. Output `FindLog -> <Log>` per run.
  4. Run `CraftWood` for each Wood needed, consuming a Log each time; track the
     outputs. Output `CraftWood -> <Wood>` per run.
  5. If a Stick is needed, run `CraftSticks` with one Wood (yields 2 Stick);
     track the outputs. Output `CraftSticks -> <Stick> x2`.
  6. Run `CraftWoodPick` with one Wood + one Stick. Output `CraftWoodPick -> <WoodPick>`.
  7. On any failed run, output its error and stop.

For deeper targets, extend the recipe table downward and add a have/need + run
loop per intermediate class, in dependency order (leaves first).

## Anti-example -- do NOT do this

    ---
    name: show-object
    description: prints an object
    ---

    # show-object

    print the object nicely

Why it fails: no named tools, no output format, no error handling; the
description just echoes the body. Always name the tools, fix the exact output,
and handle errors -- pick the picker or the argument pattern, never echo the
user's prose into the body.
