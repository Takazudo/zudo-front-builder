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
zfb.config.json        zfb configuration
pages/
  index.tsx            home page (lists posts)
  posts/[slug].tsx     per-post page
content/
  posts/
    hello.md           sample post
```

## Adding content

Drop `.md` or `.mdx` files into `content/posts/` — each file becomes a post
with a URL at `/posts/<slug>`.

## Configuration

See `zfb.config.json` and the [zfb docs](https://github.com/Takazudo/zudo-front-builder)
for available options.
