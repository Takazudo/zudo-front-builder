---
title: Hello from node-free
date: 2026-01-01
description: A sample post for the node-free template.
---

Welcome to your **node-free** zfb site.

This post lives under `content/posts/` as a plain Markdown file. zfb reads
it at build time and makes it available via the `posts` content collection.

## No Node, no pnpm

This template is designed for the `zfb` binary alone. There is no
`package.json` and no `pnpm install` step. Run:

```sh
zfb dev    # local development server
zfb build  # production build → dist/
```
