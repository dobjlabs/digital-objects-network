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

Use the first supported client path below.

#### Codex in-app browser

1. Start this command as a long-running background process, replacing `<HOME>`
   with the absolute home directory path. If port 7719 is already serving the
   dashboard, reuse it instead:

   `python3 -m http.server 7719 --bind 127.0.0.1 --directory <HOME>/.dobj/dashboard`

2. Open `http://127.0.0.1:7719/` in the Codex in-app browser, make the browser
   visible, verify that the `Digital Objects` heading rendered, and leave the
   dashboard tab open.
3. Output exactly one line: `dashboard -> http://127.0.0.1:7719/  (pane open)`.

#### Claude Preview

1. Merge this configuration into the project-local `.claude/launch.json`
   (create the file if absent; keep any existing configurations). Replace
   `<HOME>` with the absolute home directory path:

   { "name": "dobj-dashboard", "runtimeExecutable": "python3",
   "runtimeArgs": ["-m", "http.server", "7719", "--directory", "<HOME>/.dobj/dashboard"],
   "port": 7719 }

2. Call `preview_start` with `{name: "dobj-dashboard"}`.
3. Output exactly one line: `dashboard -> http://127.0.0.1:7719/  (pane open)`.

If neither client path is available, output exactly one line instead:
`dashboard -> open ~/.dobj/dashboard/index.html in your browser`.

### STOP

Use the active client's path:

- Codex: close the in-app browser tab at `http://127.0.0.1:7719/` and stop the
  background static server started by this command.
- Claude Preview: call `preview_list`, find the entry named `dobj-dashboard`,
  and call `preview_stop` with its `serverId`.

Output exactly one line: `dashboard stopped` -- or `no dashboard to stop` if
there is no open dashboard or server.
