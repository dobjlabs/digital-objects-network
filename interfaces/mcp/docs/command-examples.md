# Command body templates

Templates for the README body you draft in `create-command`, plus an
anti-example. Action names below are from the bundled `craft-basics` plugin
(`FindLog`: () -> Log; `CraftWood`: Log -> Wood); get the real `pluginName` and
action names for the loaded plugin from `list_actions`.

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
    4. Call `inspect_object` with that object's `fileName`. Output its class,
       status, and fields, one per line. On error, output the error verbatim. Stop.

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
    2. Call `inspect_object` with the argument as `fileName`. Output its fields,
       one per line. On error, output the error verbatim. Stop.

## Pattern C -- multi-step planner (reach a target)

Walk the recipe backwards: check `list_objects`, run only the missing actions.
Running an action is async -- `run_action` returns a `runId`; poll
`get_run(runId)` until `succeeded`, then read produced objects from `result`.

    ---
    name: make-wood
    description: Make a Wood -- find a Log if needed, then craft Wood.
    ---

    # make-wood

    ## Output rules
    - Plain text. One line per action run (`<Action> -> <new object>`). No markdown.

    ## Steps
    1. Call `list_objects`; count live Logs and Woods.
    2. If there is no live Wood and no live Log: `run_action` `FindLog`
       (`{pluginName, name: "FindLog"}`, no inputs), poll `get_run` until
       `succeeded`, output `FindLog -> <new Log>`.
    3. If there is no live Wood: `run_action` `CraftWood` with the Log as input,
       poll `get_run`, output `CraftWood -> <new Wood>`.
    4. On any failed run, output its error and stop.

## Anti-example -- do NOT do this

    ---
    name: show-object
    description: prints an object
    ---

    # show-object

    print the object nicely

Why it fails: no named tools, no output format, no error handling; the
description just echoes the body. Always name the tools, fix the exact output,
and handle errors -- pick the picker pattern OR the argument pattern, never echo
the user's prose into the body.
