Open the live Digital Objects dashboard. The daemon serves it at
http://127.0.0.1:7718/ and the page refreshes itself.

If the Claude Preview tool (`mcp__Claude_Preview__preview_start`) is available
(Claude Code), surface it as a pane:

1. Ensure a project-local `.claude/launch.json` has a configuration named
   `dobj-dashboard` with `"port": 7718` (the daemon already serves that port).
   For the command, use a harmless keep-alive suited to the OS -- on
   macOS/Linux, `runtimeExecutable` `"sh"` with `runtimeArgs`
   `["-c", "while :; do sleep 3600; done"]`. Merge into any existing
   configurations; do not clobber them.
2. Call `preview_start` with `{name: "dobj-dashboard"}`. The daemon is already
   listening on 7718, so this just opens the pane onto it.
3. On success, reply with one line: `dashboard pane open -> http://127.0.0.1:7718/`
   On any error, fall through to the line below.

Otherwise (no preview tool), reply with exactly one line and stop:

dashboard -> http://127.0.0.1:7718/  (open this in your browser)

To close the pane later, use the `preview-stop` command.
