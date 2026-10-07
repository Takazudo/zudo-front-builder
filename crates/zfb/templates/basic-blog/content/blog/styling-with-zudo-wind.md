---
title: How this site is styled
date: 2026-04-21
description: How the project-owned wind tokens, dark theme, and markdown CSS fit together.
tags:
  - zudo-wind
  - css
---

This starter begins with the Everyday design from the
[Design system playground](https://zfb.takazudomodular.com/docs/playground/design-system/).
Its values are yours, not engine defaults. `styles/design-system.css` owns the
colour, type, spacing, reading measure and corner decisions. `design-tokens.json`
names their semantic roles; `zfb.config.ts` merges them with the existing
Markdown showcase tokens, reset, breakpoint and theme binding. The binary
compiles utilities during a build, with no styling package to install.

## Use roles for repeated decisions

The primary-action chain is `--ds-brand` → `--ds-accent` → `bg-accent`.
Use it for the Read the posts action, selected states and keyboard focus;
keep supporting text and card borders neutral with `text-muted` and
`border-border`. The on-accent label colour is an explicit owned value too:
review its contrast when changing the brand colour.

Horizontal and vertical spacing describe different relationships.
`px-hsp-card` gives cards 24px side padding, `gap-x-hsp-gutter` names the
independent 16px column gutter, `py-vsp-stack` and `gap-y-vsp-stack` use
16px inside groups, and `mt-vsp-section` separates sections by 40px.
Changing one axis should not silently change the other.

`text-body` uses 16px with a 1.65 line height; `text-heading` and
`text-section` establish hierarchy with sans headings. `max-w-reading`
limits prose to 60ch. `rounded-panel` gives cards and actions the same 8px
corner rule. `DESIGN-NOTES.md` keeps these short usage rules beside the files.

Edit the CSS values first. Rename or add tokens in `design-tokens.json`
when your roles change; keep full class literals in the components. You can
export another design from the playground and merge its token categories
with this project. The initializer currently chooses a site template, not a
design seed.

## Complete class names in source

The compiler reads complete class candidates in pages, layouts, components,
and content. Write each class name as a full literal. For example,
`components/callout.tsx` keeps each alert's complete colour classes in its
`SPECS` map. An assembled fragment cannot identify a colour for the compiler.

## Markdown uses authored CSS

Rendered Markdown has ordinary elements such as headings, lists, and tables.
The `.prose` rules in `styles/global.css` style those elements inside the
article body. The page opts in with `class="prose mt-vsp-stack"`; `prose` is an
authored class named in `wind.authoredClasses`. Its neutral colours read the
`--zw-color-*` properties generated from the palette in the config.

The global stylesheet imports `design-system.css` and defines code panel
colours and selection styling. The `.prose` reading rhythm consumes the same
body size, line height and measure as the utility classes.

## The theme follows the toggle

The toggle writes `data-theme="dark"` on `<html>`, and the inline bootstrap
restores the saved choice before the first paint. `wind.dark` binds the
`dark:` utility variant to that attribute. The authored `.prose` selectors
also read it, so article content and page chrome change together.

The local dark values in `styles/global.css` override the semantic variables,
so the new surfaces and actions follow the same toggle. The original showcase
palette remains available for alerts, code and Markdown examples.
