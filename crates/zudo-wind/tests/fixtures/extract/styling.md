---
title: How this site is styled
date: 2026-04-21
description: Where the Tailwind setup lives, how dark mode is wired, and why markdown gets its own CSS.
tags:
  - tailwind
  - css
---

Styling here is split in two, and the split is deliberate.

## Page chrome uses utilities

Anything you can see in a `.tsx` file — the header, the post list, this
page's shell — is styled with Tailwind utility classes written inline. There
is no stylesheet to keep in sync with the markup, and no class names to
invent. zfb runs Tailwind for you; nothing needs to be installed or
configured to make it work.

Tailwind scans `pages/`, `layouts/`, `components/`, `content/`, and `src/` for
class names. That last point matters more than it looks: a class assembled at
runtime, like a template string built from a variable, is never found by the
scanner and never generated. `components/callout.tsx` keeps a full utility
string per alert variant for exactly this reason.

## Markdown gets a stylesheet

Rendered markdown produces plain `<h2>`, `<p>`, and `<table>` elements. No
utility class can reach them, so `styles/global.css` defines a `.prose` block
once, and `pages/blog/[slug].tsx` opts into it by wrapping the rendered body:

```tsx title="pages/blog/[slug].tsx"
<div class="prose mt-8">
  <post.Content components={{ ...defaultComponents }} />
</div>
```

That block is also where the markdown features get their visual treatment —
alert colours, footnote separators, task-list checkboxes, code titles, and
the highlighted lines and words that `codeEnrichment` marks up.

## Dark mode keys off an attribute

The theme toggle writes `data-theme="light"` or `data-theme="dark"` onto the
`<html>` element and persists the choice. An inline script in
`layouts/default.tsx` reads that value back before the first paint, so the
page never flashes the wrong theme on load.

Tailwind's `dark:` variant is pointed at the same attribute with one line at
the top of `styles/global.css`:

```css title="styles/global.css"
@custom-variant dark (&:where([data-theme="dark"], [data-theme="dark"] *));
```

Without it, `dark:` would follow the operating system setting and quietly
ignore the toggle. With it, every `dark:` utility and every
`[data-theme="dark"]` rule in the `.prose` block respond to the same switch.

## Changing the look

Start with the `@theme` block in `styles/global.css` — the accent colour is a
single token, and both the light and dark values live next to each other.
From there, the layout is one file (`layouts/default.tsx`) and the reading
column is one utility (`max-w-2xl`) on the wrapper inside it.
