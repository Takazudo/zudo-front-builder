# Downstream consumer requirements — curated summary

Read-only inventory taken on 2026-09-28. Nothing was built or run. Class tokens were extracted by a regex scanner over
class attributes and literal class maps, then validated against the CSS each repo had already emitted. Hook counts are
regex call-site counts. Treat every number as a measurement of the revision named here, not of the latest release.

Public consumers inventoried: **zudo-doc** (`zudolab/zudo-doc`, local clone at v5.25.0; the zfb docs site consumes the
published 5.27.0) and **zudo-sg** (`Takazudo/zudo-sg`, local clone at 0.1.1; npm latest 0.4.1). A third, private
consumer was measured too; only aggregate, non-identifying facts about it appear below. For the full zudo-doc
breakdown see `census-zudo-doc-requirements.md`.

## Release chain (why the docs host cannot move inside this epic)

`zfb release` → `@takazudo/zdtp` → `@takazudo/zudo-doc` major → `@takazudo/zudo-sg` → zfb `docs/` pin bump.
zudo-doc 5.x declares `peerDependencies` `@takazudo/zfb ^2.x` and `preact ^10`; `zudoDoc()` hardcodes
`framework: "preact"` and `tailwind: { enabled: true }`.

## Contracts consumers rely on today

- CSS entry shape, in every site entry: `@import "tailwindcss/preflight"` + `@import "tailwindcss/utilities"` — the
  split pair, never the full bundle. The default Tailwind theme is never loaded.
- **Utilities are deliberately UNLAYERED** so they beat layered authored rules; preflight sits in the lowest named
  layer. The handoff's proposed `@layer zw-utilities` inverts this.
- zudo-doc package CSS import order is load-bearing: theme → safelist → content → page-loading → features, then the
  consumer's own override block.
- Package-shipped candidate safelists: a single `@source inline("...")` line generated from compiled dist JS
  (zudo-doc about 2,690 tokens, of which only about 480 are real utilities; zudo-sg about 790).
- `zfb css --input … --output … --source … --no-auto-source` is called from zudo-doc's package build.
- `Island` is used both as JSX and as a plain function call returning `{ type, props, key }`.
- DOM attributes read or written directly by consumers: `data-zfb-island`, `data-zfb-island-skip-ssr`,
  `data-zfb-island-mounted`, `data-zfb-island-remount`, `data-zfb-transition-persist`, `data-zfb-reload`, `data-props`.
- Router events listened to: `zfb:after-swap`, `zfb:before-preparation`, `zfb:before-swap`, `zfb:page-load`.
- Island discovery by the top-of-file `"use client"` directive; published packages ship per-file compiled JS to keep it.

## Utility language: what real code uses beyond the handoff's v1

| Feature | zudo-doc | zudo-sg | Note |
| --- | --- | --- | --- |
| Distinct utility tokens | ~550 | ~450 | the private consumer is about 3× larger |
| Named spacing tokens among them | 119 | 86 | numeric scale (`p-4`) is effectively unused: no bare `--spacing` is defined |
| `dark:` variant | 0 | 0 | theming is done with custom properties |
| group-* / peer-* | 56 + 4 occurrences | 34 + 8 | zudo-sg uses named groups (`group/ctx`) and `group-data-[..]` |
| Pseudo / structural variants | first, last, focus-within, placeholder, backdrop | before, after, marker, placeholder, backdrop, first | |
| Arbitrary selector variants | 6 distinct / 13 occurrences | 7 / 9 | all target descendants of injected HTML or a details marker |
| Colour opacity modifier `/N` | 17 tokens | 5 | values 5, 10, 30, 50, 60, 80 |
| Underscore-to-space arbitrary values | 5 | 9 | grid templates, `color-mix()`, shadows |
| Arbitrary properties | 1 (zfb docs playground) | 2 | |
| Stacked variants | 0 | 2 | |
| `max-*` breakpoints, `aria-*` | 0 | `max-sm` 19, `aria-pressed` 3 | |
| Arbitrary values overall | 104 distinct | 61 | lengths, `calc()`, `var()`, `clamp()`, hex colours |
| Tailwind directives `@apply` / `@utility` / `@plugin` / `@custom-variant` | 0 | 0 | the private consumer uses `@apply` and `@utility` |

Families the handoff does not name but real code needs: transition (+ duration), transform (rotate, translate),
outline (the focus-ring system), ring, shadow, opacity, cursor / pointer-events / user-select / resize / accent-color,
z-index as named tiers, overflow / overscroll, `sr-only`, list-style, `space-y-*`, aspect-ratio, `tabular-nums`,
`size-*`, fractions (`w-1/2`), `subgrid`.

Token sources are CSS, not config: `@theme`, `@theme static`, `@theme inline`, plus a `--color-*: initial` namespace
wipe and a later consumer override block (this override block is zudo-doc's public customisation API). Breakpoints
are declared in px (640 / 1024 / 1280).

Reset: every entry imports the full Tailwind preflight and depends on it (universal margin reset, border-style
defaults behind the `border` utility, list-marker removal, root font family).

Authored CSS keyed on utility class names exists (`.hidden:has(> …)`, `a.group.block.rounded`, `h1.text-heading` …),
so those class names must stay in markup.

Latent defects a strict engine will surface — classes written today that emit nothing because no matching theme key
exists: `rounded` (62 sites across zudo-doc and the zfb docs playground), `ease-in-out`, `animate-spin`,
`animate-pulse`, `max-w-sm`, `rounded-md`, `shadow-md`, `py-hsp-3xs`, `2xl:w-[24px]`, `md:grid-cols-2`.

Classes also live inside inline vanilla-script strings (`classList.add(...)`), so the extractor must read string
literals in `.ts` files.

## Runtime: what real code uses beyond the handoff's v1

| Feature | zudo-doc | zudo-sg |
| --- | --- | --- |
| Built-in hook calls | 167 (useState 54, useEffect 52, useRef 24, useCallback 24, useMemo 13) | 56 |
| Effects that re-run on state (non-empty deps) | 31 of 52 | 4 of 23 |
| Raw HTML insertion sites | 38 (20 are inline `<script>`) | 14 |
| Conditional mounts in interactive files | 50 `&&` + 18 ternary | 12 + 3 |
| Keyed rendering in interactive files | 32 keys / 37 maps | 9 / 10 |
| `<select>` / radio | 5 / 4 | 2 / 0 |
| Skip-SSR client-only islands (`ssrFallback`) | 9 call sites | several |
| Context, portals, forwardRef, Suspense, lazy, class components, signals packages | 0 | 0 |
| IME composition guards | 1 | 0 |

Additional shapes that must work: components called as plain functions; hand-built element literals in `.ts`
files; child introspection (`toChildArray`, `cloneElement`); a string-valued `onload` attribute emitted verbatim on
the server; custom-element tags; SVG with both kebab-case and camelCase attributes; both `class` and `className`
spellings; style objects and style strings; large JSON island props; persisted regions whose nested islands keep
their DOM; dispose-and-recreate on navigation with refreshed props.

The private consumer additionally uses context and portals. zudo-doc shows portals are avoidable with the native
`<dialog>` element.

zudo-sg has two unique shapes: it embeds its own Tailwind compiler for preview CSS (so it needs a callable
standalone compile entry), and its story contract re-runs `render(args)` closures on every control change, which is
the opposite of setup-once.

## Test exposure in consumers

About 100 zudo-doc test files and 84 zudo-sg test files import Preact tooling; most use a server
render-to-string entry. A `renderToString` export and an awaitable flush helper are what they will port to.
