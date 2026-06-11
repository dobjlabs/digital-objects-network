# view

Open or close the live Digital Objects dashboard. The daemon writes the
dashboard files to `~/.dobj/view/` on startup.

## Output rules

- Plain text. The only output is one of the result lines below -- no preamble,
  no commentary, no markdown.

## Steps

Pick the action from the argument: "stop", "close", or "off" -> STOP; anything
else (including no argument) -> START.

### START

1. Merge this configuration into the project-local `.claude/launch.json` (create
   the file if absent; keep any existing configurations). Replace `<HOME>` with
   the absolute home directory path:

   { "name": "dobj-view", "runtimeExecutable": "python3",
     "runtimeArgs": ["-m", "http.server", "7719", "--directory", "<HOME>/.dobj/view"],
     "port": 7719 }

2. Call `preview_start` with `{name: "dobj-view"}`.
3. Output exactly one line: `view -> http://127.0.0.1:7719/  (pane open)`.
   If the Claude Preview tool is unavailable or `preview_start` errors, output
   exactly one line instead: `view -> open ~/.dobj/view/index.html in your browser`.

### STOP

1. Call `preview_list`, find the entry named `dobj-view`, and call `preview_stop`
   with its `serverId`.
2. Output exactly one line: `view stopped` -- or `no view to stop` if there is
   no such entry.
