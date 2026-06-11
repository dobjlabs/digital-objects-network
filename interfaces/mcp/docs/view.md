Open or close the live Digital Objects dashboard. The daemon writes the
dashboard files to `~/.dobj/view/` on startup.

Decide from the argument: "stop", "close", or "off" -> STOP; anything else
(including no argument) -> START.

## START

1. Merge this configuration into the project-local `.claude/launch.json` (create
   the file if absent; keep any existing configurations). Replace `<HOME>` with
   the absolute home directory path:

   {
     "name": "dobj-view",
     "runtimeExecutable": "python3",
     "runtimeArgs": ["-m", "http.server", "7719", "--directory", "<HOME>/.dobj/view"],
     "port": 7719
   }

2. Call `preview_start` with `{name: "dobj-view"}`. This launches the static
   server and opens the pane at http://127.0.0.1:7719/.
3. Reply with one line: `view -> http://127.0.0.1:7719/  (pane open)`.

If the Claude Preview tool is unavailable, or `preview_start` errors, reply with
exactly one line and stop:

view -> open ~/.dobj/view/index.html in your browser

## STOP

Call `preview_list`, find the entry named `dobj-view`, and call `preview_stop`
with its `serverId`. Reply with one line: `view stopped`. If there is no such
entry, reply: `no view to stop`.
