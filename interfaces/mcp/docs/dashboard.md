# dashboard

Open or close the live Digital Objects dashboard. The daemon writes the
dashboard files to `~/.dobj/dashboard/` on startup.

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

   { "name": "dobj-dashboard", "runtimeExecutable": "python3",
   "runtimeArgs": ["-m", "http.server", "7719", "--directory", "<HOME>/.dobj/dashboard"],
   "port": 7719 }

2. Call `preview_start` with `{name: "dobj-dashboard"}`.
3. Output exactly one line: `dashboard -> http://127.0.0.1:7719/  (pane open)`.
   If the Claude Preview tool is unavailable or `preview_start` errors, output
   exactly one line instead: `dashboard -> open ~/.dobj/dashboard/index.html in your browser`.

### STOP

1. Call `preview_list`, find the entry named `dobj-dashboard`, and call
   `preview_stop` with its `serverId`.
2. Output exactly one line: `dashboard stopped` -- or `no dashboard to stop` if
   there is no such entry.
