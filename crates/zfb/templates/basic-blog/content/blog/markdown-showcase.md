---
title: Markdown feature showcase
date: 2026-04-22
description: One live example of every markdown feature this starter enables, with links to the docs.
tags:
  - markdown
  - reference
---

This post renders one minimal example of each markdown feature turned on in
`zfb.config.ts`. Read it beside that file: everything below is a direct
consequence of a key in the `markdown` block.

## TOC

## Tables

GFM tables are on by default — no config needed. See the
[GFM constructs docs](https://zfb.takazudomodular.com/docs/markdown-features/gfm).

| Feature          | Config key                              | Enabled here |
| ---------------- | --------------------------------------- | ------------ |
| Tables           | `markdown.gfm.table`                    | default on   |
| Strikethrough    | `markdown.gfm.strikethrough`            | default on   |
| Autolink literals | `markdown.gfm.autolinkLiteral`         | default on   |
| Task lists       | `markdown.gfm.taskListItem`             | opted in     |
| Footnotes        | `markdown.gfm.footnoteDefinition`       | opted in     |
| Alerts           | `markdown.features.githubAlerts`        | opted in     |
| Code enrichment  | `markdown.features.codeEnrichment`      | opted in     |
| Heading-marker TOC | `markdown.features.headingMarkerToc`  | opted in     |

## Strikethrough

Wrapping text in double tildes marks it as removed: ~~this sentence was cut~~.
Like tables, it is on unless you disable it.

## Autolink literals

A bare URL becomes a link on its own, no angle brackets needed:
https://zfb.takazudomodular.com. Also on unless you disable it.

## Task lists

Task lists need `taskListItem: true`, because the conservative GFM default
leaves them off. Checkboxes render disabled — they record state, they are not
inputs.

- [x] Enable the GFM constructs this site needs
- [x] Write one example per feature
- [ ] Delete this post and write your own

## Footnotes

Footnotes also need opting in via `footnoteDefinition: true`.[^why] Each
reference links to a collected section at the end of the page, and each
definition links back.

[^why]: The GFM default is deliberately conservative so that turning zfb's markdown pipeline on cannot change how an existing document parses.

## Alerts

`githubAlerts` rewrites GitHub-style alert blockquotes into components. The
five types are `NOTE`, `TIP`, `IMPORTANT`, `WARNING`, and `CAUTION` — see the
[GitHub alerts docs](https://zfb.takazudomodular.com/docs/markdown-features/github-alerts).

> [!NOTE]
> The component behind each type lives in `components/callout.tsx` and is
> wired up in `mdx-components.tsx`. Restyle it there.

> [!WARNING]
> A `[!TYPE]` prefix must be alone on its line. `> [!NOTE] My title` is not an
> alert — it stays an ordinary blockquote.

## Code blocks

Syntax highlighting and heading anchor links are always on: no config key
enables them, and every `##` heading above already has an `id` you can link
to. `codeEnrichment` adds the three annotations below — see the
[code enrichment docs](https://zfb.takazudomodular.com/docs/markdown-features/code-enrichment).

A `title="…"` token in the fence renders a filename bar above the block. That
one is core behaviour, documented under
[code block title](https://zfb.takazudomodular.com/docs/markdown-features/code-title).

```ts title="lib/greet.ts"
export function greet(name: string) {
  return `Hello, ${name}`;
}
```

A brace-delimited range after the language highlights those lines — here,
line 2.

```ts {2}
const posts = await getCollection("blog");
const latest = posts.at(-1);
console.log(latest?.data.title);
```

Trailing `[!code ++]` and `[!code --]` comments mark added and removed lines.
The marker comment itself is stripped from the output.

```ts
const posts = await getCollection("blog");
const sorted = posts.sort(byDate); // [!code --]
const sorted = [...posts].sort(byDate); // [!code ++]
```

Slash-delimited phrases in the fence metadata emphasise every occurrence of
that phrase in the visible code.

```ts /slug/
export async function paths() {
  const posts = await getCollection("blog");
  return posts.map((post) => ({ params: { slug: post.slug } }));
}
```

## Table of contents

The list under the `TOC` heading near the top of this page was generated. The
`headingMarkerToc` feature watches for a heading whose text matches the
configured anchor — `TOC` by default — and inserts links to the headings that
follow. See the
[heading-marker TOC docs](https://zfb.takazudomodular.com/docs/markdown-features/heading-marker-toc).

## What is not enabled

zfb ships more opt-in features than this starter uses — code tabs, ruby
annotations, mermaid diagrams, image dimensions, link validation,
transclusion, and others. The `README.md` in the project root lists them with
their config keys, and the
[markdown features index](https://zfb.takazudomodular.com/docs/markdown-features)
is the complete map.
