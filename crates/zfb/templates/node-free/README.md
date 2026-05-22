# node-free zfb site

A minimal zfb project that requires **only the `zfb` binary** — no Node, no
pnpm, no `package.json`.

## Getting started

```sh
# Start the dev server
zfb dev

# Build for production (output → dist/)
zfb build
```

No `pnpm install`, no `pnpm dev`, no `pnpm exec` needed.

## Structure

```
zfb.config.json   zfb configuration (JSON only — no .ts in this template)
pages/
  index.tsx       single home page
```

## Adding pages

Drop more `.tsx` files into `pages/`. Each becomes a route at the same path
under `/` (so `pages/about.tsx` → `/about`).

## Content collections (currently Node-only)

Content collections (`getCollection("posts")` etc.) and Markdown sources under
`content/` currently require Node-only `node:fs` at build time. The Node-free
template intentionally omits them. Add them back once the upstream gap closes
(tracking: [#392](https://github.com/Takazudo/zudo-front-builder/issues/392),
[Tier 2 epic #390](https://github.com/Takazudo/zudo-front-builder/issues/390)).

## Configuration

See `zfb.config.json` and the
[zfb docs](https://github.com/Takazudo/zudo-front-builder) for available
options.
