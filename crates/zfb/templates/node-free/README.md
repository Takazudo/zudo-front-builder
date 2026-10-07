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

## Styling

The starter ships the Everyday design as editable project files.
`styles/design-system.css` owns colour, type, independent horizontal/vertical
spacing, a 60ch reading measure and 8px corners. `styles/global.css` imports
those values and styles the static pages and Markdown. `zfb.config.json`
contains an explicit Wind section matching `design-tokens.json`; keep both
in sync when renaming a utility role. `components/design-rules.ts` supplies the home page
explanations; update it and
`DESIGN-NOTES.md` when changing values. The notes explain usage.

The home page and post cards use semantic utilities such as `bg-surface`,
`px-hsp-card`, `py-vsp-stack`, and `rounded-panel`. zfb compiles them inside
the binary, with no Node or install step. The browser output stays static.
Try alternatives in the [Design system playground](https://zfb.takazudomodular.com/docs/playground/design-system/)
and merge the files manually; the initializer has no design-seed selector.

This starter intentionally emits a Wind stylesheet. The engine itself still
has no default palette: an empty configuration without CSS or class candidates
emits no stylesheet. `"wind": false` disables utility generation while keeping
authored CSS.

## Structure

```
zfb.config.json     zfb configuration (JSON form; see note below on .ts support)
styles/
  design-system.css  owned Everyday design values
  global.css         imports values and styles static Markdown
design-tokens.json   semantic Wind role reference
components/design-rules.ts    current rules displayed on the home page
DESIGN-NOTES.md      short usage rules
pages/
  index.tsx         home page — lists posts from the collection
  about.md          sample .md page entry — renders at /about
  posts/[slug].tsx  dynamic per-post page
content/
  posts/            posts collection (Markdown with frontmatter)
    hello.md
    second.md
```

## Adding pages

Drop more `.tsx` files into `pages/`. Each becomes a route at the same path
under `/` (so `pages/about.tsx` → `/about`).

You can also add `.md` files for simple content pages. `pages/about.md`
produces the same `/about` route and is compiled through the MDX pipeline.
Two frontmatter keys are recognised: `title` (sets `<title>`) and `lang`
(sets `<html lang="…">`; defaults to `"en"`). No layout system is available
for `.md` pages in v1 — use `.tsx` if a shared layout is needed. See the
[about page](/about) in this template for a working example.

Pre-authored static HTML files can be placed as `.html` pages (e.g.
`pages/contact.html` → `/contact`). The file must be a complete HTML document
and is copied verbatim to `dist/` without any post-processing.

## Content collections

`getCollection("posts")` reads `.md` files under `content/posts/` at build
time. The Markdown frontmatter (between `---` lines) is parsed into the
entry's `data` field; the body is plain text on `entry.body`.

```tsx
// pages/index.tsx
import { getCollection } from "zfb/content";

export async function getStaticProps() {
  const posts = await getCollection("posts");
  return { props: { posts } };
}
```

Add another post by dropping a new `.md` file into `content/posts/` and
editing the `collections` entry in `zfb.config.json` (or `zfb.config.ts` —
see below) if you want a different name.

## Configuration

Both `zfb.config.json` and `zfb.config.ts` are supported — no Node required
for either. When `zfb.config.ts` is present it takes precedence; the TypeScript
file is bundled and evaluated by the in-process V8 engine embedded in the `zfb`
binary. This project ships the `.json` form for simplicity; rename it to `.ts`
and use `defineConfig` from `"zfb/config"` if you prefer editor types:

```ts
import { defineConfig } from "zfb/config";

export default defineConfig({
  collections: [{ name: "posts", path: "content/posts" }],
});
```

See the [zfb docs](https://github.com/Takazudo/zudo-front-builder) for all
available options.
