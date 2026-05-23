---
title: About
lang: en
---

## About this site

This is the **node-free** zfb template — a minimal site that requires only
the `zfb` binary. No Node.js, no pnpm, no `package.json`.

This page is a `.md` page entry. Drop a Markdown file into `pages/` and
it becomes a route, just like a `.tsx` page.

## How it works

`pages/about.md` maps to the route `/about`. The engine compiles the
Markdown body through the MDX pipeline and wraps it in a minimal HTML
shell using the `title` and `lang` frontmatter values.

## Back to home

[Go to the home page](/).
