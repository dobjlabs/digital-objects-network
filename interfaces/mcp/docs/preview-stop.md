Close the Digital Objects dashboard pane. The daemon keeps serving the dashboard
at http://127.0.0.1:7718/ -- this only closes the Claude Code pane.

If the Claude Preview tool is available: call `preview_list`, find the entry
named `dobj-dashboard`, and call `preview_stop` with its `serverId`. Reply with
one line: `dashboard pane closed`.

Otherwise, or if there is no such pane, reply with exactly one line:

no dashboard pane to close
