Open or close the live Digital Objects dashboard. The daemon serves it at
http://127.0.0.1:7718/.

Choose the action from the input/argument:
- "stop", "close", "hide", "off" -> STOP
- anything else, including no argument (e.g. "start", "open", "show") -> START

## START

The daemon already serves the dashboard at http://127.0.0.1:7718/.

If the Claude Preview tool (`mcp__Claude_Preview__preview_start`) is available
(Claude Code):

1. Ensure a project-local `.claude/launch.json` has a configuration named
   `dobj-view` with `"port": 7718` and a harmless keep-alive command suited to
   the OS -- on macOS/Linux, runtimeExecutable `"sh"`, runtimeArgs
   `["-c", "while :; do sleep 3600; done"]`. Merge into any existing
   configurations; do not clobber them.
2. Call `preview_start` with `{name: "dobj-view"}` to open the pane onto it.
3. On success reply with one line: `view -> http://127.0.0.1:7718/  (pane open)`.
   On any error, fall through to the line below.

Otherwise reply with exactly one line and stop:

view -> http://127.0.0.1:7718/  (open this in your browser)

## STOP

If the Claude Preview tool is available: call `preview_list`, find the entry
named `dobj-view`, and call `preview_stop` with its `serverId`. The daemon keeps
serving the dashboard; this only closes the pane. Reply with one line:
`view stopped`.

Otherwise, or if there is no such pane, reply with exactly one line:

no view to stop
