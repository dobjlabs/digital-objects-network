# zk-craft docs

Documentation site for [zk-craft](../README.md), built with [Vocs](https://vocs.dev).

## Install

```bash
pnpm install
```

## Run

```bash
pnpm dev      # local dev server at http://localhost:5173
pnpm build    # static build → ./dist
pnpm preview  # serve the built site
```

## Layout

```
pages/        # one .mdx per route — sidebar wired up in vocs.config.ts
public/       # static assets (favicon, logo, images)
vocs.config.ts
```

To add a page: drop a new `.mdx` under `pages/`, then add an entry to the `sidebar` in [vocs.config.ts](vocs.config.ts).
