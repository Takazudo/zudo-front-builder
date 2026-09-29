---
title: How this site is styled
date: 2026-04-21
description: How the project-owned wind tokens, dark theme, and markdown CSS fit together.
tags:
  - zudo-wind
  - css
---

This starter keeps its design values in `zfb.config.ts`. The `wind` block
selects the `owned-v1` reset and declares the colour palette, spacing unit,
type scales, and the `sm` breakpoint. Page components use those values through
utility classes. The binary compiles them during a build, with no styling
package to install.

## Complete class names in source

The compiler reads complete class candidates in pages, layouts, components,
and content. Write each class name as a full literal. For example,
`components/callout.tsx` keeps each alert's complete colour classes in its
`SPECS` map. An assembled fragment cannot identify a colour for the compiler.

## Markdown uses authored CSS

Rendered Markdown has ordinary elements such as headings, lists, and tables.
The `.prose` rules in `styles/global.css` style those elements inside the
article body. The page opts in with `class="prose mt-8"`; `prose` is an
authored class named in `wind.authoredClasses`. Its neutral colours read the
`--zw-color-*` properties generated from the palette in the config.

The same stylesheet defines the accent for links, code panel colours, and
selection styling. Change those authored values there; change the utility
palette and scales in `zfb.config.ts`.

## The theme follows the toggle

The toggle writes `data-theme="dark"` on `<html>`, and the inline bootstrap
restores the saved choice before the first paint. `wind.dark` binds the
`dark:` utility variant to that attribute. The authored `.prose` selectors
also read it, so article content and page chrome change together.
