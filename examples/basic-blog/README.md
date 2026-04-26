# basic-blog — zfb dogfood example

A minimal real zfb site, kept inside the `zudo-front-builder` workspace as
both a shipped example and a fixture the CLI can be smoke-tested against.

## What is here

```
examples/basic-blog/
├── content/blog/        5 markdown posts with frontmatter (title, date, description, tags)
├── components/          theme-toggle.tsx — the only client island
├── layouts/             default.tsx — shared chrome (header, footer, theme toggle)
├── pages/
│   ├── index.tsx               static — homepage listing all posts
│   ├── blog/[slug].tsx         dynamic — one route per post
│   ├── blog/page/[page].tsx    dynamic — paginated index, pageSize 3
│   └── tags/[tag].tsx          dynamic — one route per unique tag
├── styles/global.css    light/dark theme via CSS variables + [data-theme="dark"]
├── zfb.config.future.ts canonical TS form (aspirational — see below)
├── zfb.config.json      what the v0 CLI actually loads
├── package.json
├── tsconfig.json
└── .gitignore
```

Cross-references into the docs:

- [/getting-started/your-first-site](../../docs/src/content/docs/getting-started/) — narrative walkthrough.
- [/concepts/routing](../../docs/src/content/docs/) — file-based routing rules.
- [/api/get-collection](../../docs/src/content/docs/) — `getCollection("blog")`.
- [/api/paginate](../../docs/src/content/docs/) — the pagination helper used in `blog/page/[page].tsx`.
- [/architecture/js-runtime](../../docs/src/content/docs/) — the JS runtime decision (ADR-001) that gates real SSR.

## v0 status — read this first

Today, `zfb build` emits **per-route stub HTML** rather than fully rendered
pages. The real per-route renderer is blocked on the JS runtime decision
(ADR-001) — see `/architecture/js-runtime` in the docs site. The example is
structured correctly and is ready for the renderer; right now `zfb build`
in this directory will:

- Write a single stub at `dist/index.html` for the static `/` route.
- Warn-skip every **dynamic** route — `[slug]`, `page/[page]`, and
  `tags/[tag]` — because each needs its `paths()` export evaluated by the
  JS runtime to expand into concrete URLs.

So the v0 build output here is just one HTML file. Once the renderer
lands, the same files in this directory will produce a fully rendered
blog (one post page, two paginated indexes, and one page per unique tag)
with no source changes required.

## Running it from the workspace

The CLI in this repo is a Rust binary, not a published npm package, so you
do not install zfb here. From inside `examples/basic-blog/`:

```sh
# one-shot build into ./dist (writes per-route stub HTML in v0)
cargo run -p zfb -- build

# or build into an arbitrary directory
cargo run -p zfb -- build --outdir /tmp/basic-blog-out

# dev server (port 3000)
cargo run -p zfb -- dev
```

Equivalently, from the workspace root, point the CLI at this directory:

```sh
(cd examples/basic-blog && cargo run -p zfb -- build)
```

## Why two `zfb.config.*` files

`zfb.config.future.ts` is the canonical, type-checked shape that zfb will
load once the TS config loader lands. Today the v0 CLI hard-errors if it
sees a real `zfb.config.ts` (see `crates/zfb/src/config.rs`), so the file
is parked under a `.future.ts` name to keep it out of the loader's
resolution path while still being available for review and type-checking.
`zfb.config.json` is what the v0 build actually reads. Both files describe
the same project; keep them in sync until the TS form is wired up — at
which point you can rename `zfb.config.future.ts` to `zfb.config.ts` and
delete the JSON sibling.

## Stretch goals (not in v0)

- RSS feed (`pages/rss.xml.ts`) — skipped until the renderer can return
  non-HTML responses.
- Real per-route SSR — see ADR-001.
