# Docs

[Vocs](https://vocs.dev) site for the Digital Objects Network: the network
write-up, architecture, driver install guide, and application catalog.

## Develop

```bash
pnpm install
pnpm dev     # http://localhost:5173
```

## Build

```bash
pnpm build   # static site in dist/
```

## Deploy

Deployed to GitHub Pages by `.github/workflows/docs.yml` on any push that
touches `docs/`. The workflow injects `BASE_PATH` from the Pages config, so
the build works both at `<owner>.github.io/<repo>/` and on a custom domain.

## Layout

- `pages/` - MDX pages: landing (`index.mdx`), `network`, `architecture`,
  `install`, and `applications/`
- `public/` - static assets served at the site root
- `assets/` - assets imported by pages
- `vocs.config.ts` - title, sidebar, top nav, banner, edit links

The hosted-endpoint table (landing page) and the application catalog
(`applications/index.mdx`) take community additions by PR.
