# basic-blog — zfb dogfood example

A minimal real zfb site, kept inside the `zudo-front-builder` workspace as
both a shipped example and a fixture the CLI can be smoke-tested against.

## What is here

```
examples/basic-blog/
├── content/blog/        5 posts (4 .md, 1 .mdx) with frontmatter (title, date, description, tags)
├── components/          theme-toggle.tsx — the only client island; note.tsx — admonition used inside hello-zfb.mdx
├── layouts/             default.tsx — shared chrome (header, footer, theme toggle)
├── pages/
│   ├── index.tsx               static — homepage listing all posts
│   ├── blog/[slug].tsx         dynamic — one route per post
│   ├── blog/page/[page].tsx    dynamic — paginated index, pageSize 3
│   └── tags/[tag].tsx          dynamic — one route per unique tag
├── styles/global.css    light/dark theme via CSS variables + [data-theme="dark"]
├── zfb.config.future.ts canonical TS form (recommended for new projects — see below)
├── zfb.config.json      back-compat JSON form, takes precedence in this example
├── package.json
├── tsconfig.json
└── .gitignore
```

Cross-references into the docs:

- [/getting-started/your-first-site](../../docs/src/content/docs/getting-started/) — narrative walkthrough.
- [/concepts/routing](../../docs/src/content/docs/) — file-based routing rules.
- [/api/get-collection](../../docs/src/content/docs/) — `getCollection("blog")`.
- [/api/paginate](../../docs/src/content/docs/) — the pagination helper used in `blog/page/[page].tsx`.
- [/concepts/mdx-components](../../docs/src/content/docs/) — the `<entry.Content components={...}>` rendering pattern (Sub 8 docs page).
- [/architecture/js-runtime](../../docs/src/content/docs/) — the JS runtime decision (ADR-001) that gates real SSR.

## Rendering post bodies with `entry.Content`

Each entry returned by `getCollection("blog")` carries a `Content` component
that renders the post body as JSX. The per-post route (`pages/blog/[slug].tsx`)
uses `<post.Content components={{ ...defaultComponents, Note }} />` to (a)
spread in the htmlOverrides convention from `zfb` so HTML tags emitted by
the markdown body resolve to the package's passthrough components, and (b)
inject custom JSX components — here, the `<Note>` admonition used inside
`content/blog/hello-zfb.mdx` — so MDX-only posts can reach for project-level
components by name. See `/concepts/mdx-components` (Sub 8) for the full
contract and the design rationale.

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

`zfb.config.future.ts` is the canonical, type-checked shape — the
**recommended** way to author a zfb config for new projects. It uses the
`defineConfig` helper from `zfb/config` so editors surface field-level
types and typos surface at compile time.

`zfb.config.json` is the back-compat form, kept around so projects that
predate the TS loader keep working unchanged. The loader picks JSON over
TS when both files are present, which is why this example still parks
the TS form under a `.future.ts` name — the JSON sibling stays the
source of truth for `cargo run -p zfb -- build` against this directory.
Rename `zfb.config.future.ts` to `zfb.config.ts` and delete the JSON to
flip this example onto the TS path.

The TS loader bundles `zfb.config.ts` with the staged esbuild binary
(`crates/zfb/binaries/esbuild/esbuild`, overridable via
`ZFB_ESBUILD_BIN`), then evaluates it with `node` to pull the default
export back as JSON. **zfb requires `node` in `PATH`** — true since v0
because the production renderer spawns miniflare, and the TS loader
piggybacks on the same toolchain. The user's `import { defineConfig }
from "zfb/config"` is satisfied by an internal stub at parse time, so
the project does not need the `zfb` npm package installed locally just
to be parsed.

## Stretch goals (not in v0)

- RSS feed (`pages/rss.xml.ts`) — skipped until the renderer can return
  non-HTML responses.
- Real per-route SSR — see ADR-001.
