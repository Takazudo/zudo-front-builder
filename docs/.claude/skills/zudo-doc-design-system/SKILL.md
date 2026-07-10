---
name: zudo-doc-design-system
description: Consumer-docs CSS and token rules for this zudo-doc-powered docs site. Consult before editing docs/src/styles/global.css, Tailwind classes, color tokens, or host-authored markup.
user-invocable: true
argument-hint: "[topic: tokens, colors, imports, host-markup]"
---

# zudo-doc Consumer Design System

This repo is a consumer docs site for `@takazudo/zudo-doc`, not the upstream package implementation. Package-owned routes render the header, sidebar, TOC, footer, search, doc history, and most chrome. The host owns content, config, assets, `pages/index.tsx`, and `docs/src/styles/global.css`.

## Source of Truth

- Read `docs/src/styles/global.css` before changing CSS or token usage.
- Do not follow old reference-page pointers for design-system, component-first, or color topics. Those pages no longer exist in this consumer site.
- Do not reintroduce the old host-owned route/chrome layer (`pages/lib/**`, `pages/docs/**`, `pages/[locale]/**`, `src/components/**`, `src/utils/**`, `src/hooks/**`) for package chrome changes. Change the package upstream or use supported zudo-doc host bindings.

## What `global.css` Owns

- CSS import ordering for Tailwind and package CSS.
- Explicit Tailwind `@source` globs for non-git or copied build contexts.
- Host token registrations in `@theme`: semantic colors, spacing, icon sizes, typography, radius, breakpoints, shadows, and z-index utilities.
- `:root` constants consumed by imported package CSS.

## Token Rules

- Tailwind default colors are reset with `--color-*: initial`; do not use classes such as `text-gray-500` or `bg-blue-600`.
- Prefer semantic color utilities backed by `global.css`, such as `text-fg`, `bg-bg`, `bg-surface`, `border-muted`, `text-accent`, `text-success`, `text-danger`, `text-warning`, and `text-info`.
- Use dedicated highlight tokens for distinct roles. Search result marks use `matched-keyword-bg` / `matched-keyword-fg`; warning UI uses `warning`.
- Use spacing tokens from the host scale: `hsp-*` for horizontal spacing, `vsp-*` for vertical spacing, and `icon-*` for icon dimensions.
- Use semantic type tokens (`text-micro`, `text-caption`, `text-small`, `text-body`, `text-title`, `text-heading`, `text-display`) instead of arbitrary font-size utilities when a token fits.
- Avoid arbitrary values unless the value is genuinely one-off and no existing token expresses the role.

## Import and Package CSS Rules

- Keep `@layer zd-preflight, zd-flow;` before the Tailwind and package imports.
- Keep `@import` rules near the top of `global.css`; CSS requires imports to precede normal rules.
- Keep `@import "@takazudo/zudo-doc/safelist.css";` with the package imports so Tailwind v4 sees package-emitted utilities.
- The imported package CSS (`content.css`, `features.css`, `page-loading.css`) is the source of truth for package chrome styling. Do not vendor-copy or fork those rules into this repo.

## Host Markup Rules

- For any host-authored TSX/MDX that navigates, pair `hover:underline` with `focus-visible:underline`.
- Controls such as buttons, toggles, resize handles, swatches, and close icons should use border/background/focus treatment rather than link underlines.
- Default to server-rendered Preact for host markup. Add a client island only for real interactivity.
