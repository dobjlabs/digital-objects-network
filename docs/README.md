# Docs

[mdBook](https://rust-lang.github.io/mdBook/) for the Digital Objects Network:
the network write-up, architecture, driver install guide, testnet details, and
example applications.

## Develop

Install mdBook 0.4.40, the version CI builds with:

```bash
cargo install mdbook --version 0.4.40 --locked
```

Then, from the repository root:

```bash
just docs    # mdbook serve with live reload on http://localhost:3000
```

## Build

```bash
mdbook build docs    # static site in docs/book/
```

## Deploy

Deployed to GitHub Pages by `.github/workflows/docs.yml` on any push to `main`
that touches `docs/`. Links between pages are relative, so the build works both
at `<owner>.github.io/<repo>/` and on a custom domain; the workflow sets only
the 404 page's site URL from the Pages config.

## Layout

- `book.toml` - title, theme, repository and edit links
- `src/SUMMARY.md` - the table of contents; a page not listed here is not built
- `src/` - one Markdown file per page, with the examples under `src/examples/`

The hosted-service table (`src/testnet.md`) and the examples
(`src/examples/index.md`) take community additions by PR.
