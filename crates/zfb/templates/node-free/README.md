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
zfb.config.json     zfb configuration (JSON only — no .ts in this template)
pages/
  index.tsx         home page — lists posts from the collection
  posts/[slug].tsx  dynamic per-post page
content/
  posts/            posts collection (Markdown with frontmatter)
    hello.md
    second.md
```

## Adding pages

Drop more `.tsx` files into `pages/`. Each becomes a route at the same path
under `/` (so `pages/about.tsx` → `/about`).

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
options. The `zfb.config.ts` (`.ts`) form is intentionally omitted from
this template until upstream gains an embedded-V8 path for evaluating
`.ts` configs (tracking:
[Tier 2 epic #390](https://github.com/Takazudo/zudo-front-builder/issues/390)).
