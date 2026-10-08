# Default browser UI

A normal React + TypeScript + Vite app for browsing objects, binding action
inputs, running actions, and following proofs. All driver operations use
`dobjd`'s HTTP/SSE API. No native shell or browser engine is distributed.

## Development

From the repository root:

```bash
just dev-local    # local devnet, services, dobjd, Vite
# or: just dev / just dev-remote
```

Open `http://localhost:1420`. To run only the frontend against an existing
daemon, use `just web` (or `pnpm dev` in this directory).

Development fetches and SSE streams use `/api` on the Vite origin. Vite
proxies them to `http://127.0.0.1:7717` on its host, so forwarding only port
1420 works. `VITE_DOBJD_URL` chooses a different upstream:

```bash
VITE_DOBJD_URL=http://127.0.0.1:7727 pnpm dev
```

Restart Vite after changing it. Connection errors appear with a Retry button.

## Bundled release UI

```bash
just build-dobjd
# Equivalent:
# cd interfaces/gui && pnpm install --frozen-lockfile && pnpm build
# cargo build --release -p dobjd --features bundled-ui
```

The `bundled-ui` feature embeds `dist/` into the daemon. It deliberately fails
if the frontend has not been built. Releases build the frontend first, then
compile with this feature. Installing/updating the daemon installs/updates
its UI atomically with the binary; no Node or Vite installation is needed
on the user's machine.

Run `dobj ui` to start the daemon if needed and open the default browser.
`dobj ui --no-open` prints the URL. The UI is served at
`http://127.0.0.1:7717/ui/` (using the configured daemon port).

Production asset URLs are relative, so the same build can be served from
`/ui/` by dobjd or from `/` by a separate static server. The daemon adds an
index meta tag selecting its own API origin, including a non-default port.
A standalone build defaults to `http://127.0.0.1:7717`, independently of the
frontend's host and port:

```bash
pnpm build
python3 -m http.server 8000 --bind 127.0.0.1 --directory dist
# Open http://127.0.0.1:8000
```

For a different daemon address, set `VITE_DOBJD_URL` at build time:

```bash
VITE_DOBJD_URL=http://127.0.0.1:7727 pnpm build
```

Alternatively, add `<meta name="dobjd-api-url" content="http://127.0.0.1:7727">`
to the served index's head without rebuilding. The meta tag takes precedence
over the build-time URL, so a daemon-served UI always uses its own origin even
if the build selected a different daemon. The API base is shared by fetches and
SSE streams; it never relies on a separately hosted frontend having the daemon's port.

## Browser controls

- Import external `.dobj` files with the existing browser file input.
- Drag live inventory objects into action inputs, or use the object selector.
  Bindings use daemon-managed filenames, not browser filesystem paths.
- Open Settings with its button or Ctrl/Cmd+comma.
- The objects heading exposes the directory path in its tooltip.
- Proof status and the global state root remain visible. The old native
  process CPU sampler is removed; metrics would need a daemon API.

The Zustand store is in `src/shared/state/store.ts`; the HTTP/SSE client is
in `src/shared/api/httpClient.ts`. Active runs use replayable per-run SSE
plus status polling. The global SSE stream triggers inventory refreshes
on run output/commit events; it is not a complete state-change feed.

## Alternative GUIs

The daemon ships a default GUI, not a required GUI. Fork this app or write
your own, host it with a local static or development server, and connect to
the HTTP/SSE API. No daemon asset-directory configuration is needed.

A GUI is **trusted executable code**. It can read object information exposed
by the API and execute actions. Local HTTP(S) frontend origins are trusted on
any port; remote and opaque origins are rejected. Do not run shared GUI
projects you would not trust with your objects. This is not a skin sandbox.

See [dobjd's web UI documentation](../../services/dobjd/README.md#web-ui) for
browser access restrictions.
