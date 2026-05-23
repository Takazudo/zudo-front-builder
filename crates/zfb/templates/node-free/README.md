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
zfb.config.json     zfb configuration (JSON form used in this template)
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
editing the `collections` entry in `zfb.config.json` if you want a
different name.

## Configuration

See `zfb.config.json` and the
[zfb docs](https://github.com/Takazudo/zudo-front-builder) for available
options. `zfb.config.ts` also works Node-free — the default `zfb` binary
evaluates it in-process via an embedded V8 isolate. This template ships
`zfb.config.json` as the simpler starting point; rename or add a
`zfb.config.ts` whenever you want TypeScript types or computed values.
