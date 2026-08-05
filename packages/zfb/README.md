# @takazudo/zfb

> Rust-built static-site engine for Astro and Next.js users — millisecond rebuilds, single binary.

The public SDK module for [zfb][zfb-site]: islands, content collections,
pagination, config, plugins, and frontmatter helpers. User pages reach this
package through the bare specifier `"zfb"` — the build pipeline aliases that
specifier to `@takazudo/zfb` so user TSX can write:

```tsx
import { Island } from "zfb";
```

Full documentation: <https://takazudomodular.com/pj/zudo-front-builder/>.
Source: <https://github.com/Takazudo/zudo-front-builder>.

[zfb-site]: https://takazudomodular.com/pj/zudo-front-builder/

## Install

```sh
npm install @takazudo/zfb
# or: pnpm add @takazudo/zfb
# or: yarn add @takazudo/zfb
```

## What lives here

This package is the canonical TypeScript source for the `zfb` SDK
surface. Today it covers:

- `<Island when="visible|idle|load|media" media="...">` — JSX wrapper
  that marks a region for client-side hydration. Passing `ssrFallback`
  switches to the SSR-skip marker (`data-zfb-island-skip-ssr`).
- `scheduleHydrate(target, when, fire)` — the runtime branching helper
  consumed by the hydration runtime.
- `mountIslands(manifest)`, `mountNewIslands()`,
  `cancelPendingIslands()`, and `unmountIslands(...)` — public island
  lifecycle helpers used by the generated islands runtime and by the
  client router after body swaps.
- `When`, `WHEN_VALUES`, `DEFAULT_WHEN`, `isWhen`, `resolveWhen` — type
  and runtime utilities pinning the spelling of the four modes.
- `getCollection(name)`, `getEntry(name, slug)`, and
  `parseFrontmatter(raw)` — content collection helpers exported from
  `zfb/content`. `parseFrontmatter` is part of the public SDK surface so
  consumers can write custom content loaders that reuse the v0
  frontmatter parser without re-implementing it.
- `defaultComponents` — eleven-entry per-element override map (`h2`, `h3`,
  `h4`, `p`, `a`, `strong`, `blockquote`, `ul`, `ol`, `table`, `code`)
  ported from zudo-doc's `htmlOverrides` convention. **`h1` is deliberately
  omitted** because page titles render `<h1>` from frontmatter. Each entry
  is a thin passthrough and is also exported as a named const
  (`ContentParagraph`, `ContentLink`, …) so consumers can tree-shake-import a
  single override. Spread into a `components` prop to compose with custom
  overrides:

  ```tsx
  import { defaultComponents } from "zfb";

  <entry.Content components={{ ...defaultComponents, h2: MyFancyH2 }} />
  ```
- `mergeMdxComponents(globalSlot, perCall)` — precedence merge helper for
  the MDX component map (`defaultComponents` < global slot < per-call
  overrides).
- `paginate(items, opts)`, plus `PaginatedPage<T>` / `PaginateRoute<T>` —
  exported from `zfb/paginate`.
- `defineConfig(config)` — exported from `zfb/config` for the
  `zfb.config.ts` form (the recommended way to author a zfb project's
  configuration; the back-compat `zfb.config.json` form is still
  supported).
- `definePlugin(plugin)` — identity helper exported from `zfb/plugins`
  and the root barrel so plugin authors get typed lifecycle hooks without
  changing runtime behavior.
- `clientScript(name)` — SSR helper that returns the stable URL for a
  named client-script asset.
- `slugify(input)` and `SlugAllocator` — exported from the root barrel and
  the `zfb/slugify` subpath for heading-id parity with the Rust content
  pipeline.

The package is JSX-runtime-agnostic: the `Island` component does not
import preact or react, so it works under either framework adapter
without bundling the wrong runtime. `react` is listed as a peer
dependency but is **optional** (`peerDependenciesMeta.react.optional`)
— a preact/compat-only consumer does not need `react` installed and
does not need `auto-install-peers=true`.

## Usage

```tsx
import { Island } from "zfb";
import { Counter } from "../components/Counter.tsx"; // a "use client" component

export default function Page() {
  return (
    <>
      <h1>Welcome</h1>

      {/* Hydrate immediately on page load (default). */}
      <Island>
        <Counter />
      </Island>

      {/* Hydrate during the next idle callback. */}
      <Island when="idle">
        <Counter />
      </Island>

      {/* Hydrate only when the island first scrolls into view. */}
      <Island when="visible">
        <Counter />
      </Island>

      {/* Hydrate when a CSS media query first matches. */}
      <Island when="media" media="(max-width: 720px)">
        <Counter />
      </Island>

      {/* Skip SSR for the heavy child and render a placeholder instead. */}
      <Island when="idle" ssrFallback={<div>Loading…</div>}>
        <Counter />
      </Island>
    </>
  );
}
```

## The four `when=` modes

| `when`      | Trigger                                                        | Fallback                                           |
| ----------- | -------------------------------------------------------------- | -------------------------------------------------- |
| `"load"`    | Synchronous, immediate fire after registration. **Default.**   | n/a                                                |
| `"idle"`    | `requestIdleCallback`                                         | `setTimeout(0)` when not available                 |
| `"visible"` | `IntersectionObserver`, threshold 0, first intersection only   | Immediate fire when `IntersectionObserver` is missing |
| `"media"`   | `matchMedia(media)`, first matching change only                | Immediate fire when `matchMedia` or `media` is missing |

Unknown values produce a `console.warn` in development builds and fall
back to `"load"`.

`when="media"` requires a `media` prop containing a CSS media query
string. Supplying `media` with any other `when` value is ignored and
warns in development builds.

## Build-time output

The wrapper is intentionally type-erased at the JSX boundary. It reads
the wrapped child's JSX type identity (`displayName`, then `name`, then
host tag name) and writes the component name immediately. At the call
site, `<Island when="visible"><Counter count={1} /></Island>` renders as:

```html
<div data-zfb-island="Counter" data-when="visible" data-props='{"count":1}'>
  <!-- rendered child output -->
</div>
```

`data-props` carries the wrapped child's serializable own props across
the SSR-to-hydration boundary. The wrapper omits `children`, omits the
attribute entirely when there is no useful props payload, and the runtime
falls back to `{}` when the attribute is missing or malformed.

When `ssrFallback` is supplied, the heavy child is not rendered at SSR
time. The wrapper writes `data-zfb-island-skip-ssr="ComponentName"`
instead of `data-zfb-island`, still includes `data-when` / `data-media`
and `data-props` when applicable, and renders the fallback markup inside
the wrapper. The client runtime treats that marker as a render target
rather than a hydration target.

## Runtime helper

The hydration runtime imports (or inlines) `scheduleHydrate` from this
package:

```ts
import {
  scheduleHydrate,
  mountIslands,
  mountNewIslands,
  cancelPendingIslands,
  unmountIslands,
} from "@takazudo/zfb/runtime";

for (const el of document.querySelectorAll<HTMLElement>("[data-zfb-island]")) {
  const when = el.getAttribute("data-when") ?? "load";
  scheduleHydrate(el, when, () => hydrateOne(el));
}
```

`scheduleHydrate` returns a `cancel` function that aborts the schedule
if hydration has not fired yet. After firing, calling `cancel` is a
no-op.

`mountIslands(manifest)` captures the generated island manifest and
mounts both hydrated markers (`data-zfb-island`) and SSR-skip markers
(`data-zfb-island-skip-ssr`). `mountNewIslands()` re-walks the current
document after a client-router body swap using that captured manifest.
`cancelPendingIslands()` cancels deferred `idle` / `visible` / `media`
schedules before a swap, and `unmountIslands(root, incomingBody)` runs
framework cleanup for discarded islands while preserving matching
`data-zfb-transition-persist` islands.

## Markdown / GFM config

`ZfbConfig.markdown.gfm` controls which GitHub-Flavored-Markdown
constructs the MDX parser recognises. The field accepts three shapes:

1. **Shorthand boolean** — turn every GFM construct on or off in one
   step. Use this when you want the full GFM surface.

   ```ts
   // zfb.config.ts
   import { defineConfig } from "zfb/config";

   export default defineConfig({
     markdown: {
       gfm: true, // strikethrough + table + autolink-literal + task-list-item + footnote-definition
     },
   });
   ```

2. **Partial object** — toggle individual constructs. Fields you omit
   fall back to the conservative default (`strikethrough: true`,
   `table: true`, everything else off).

   ```ts
   // zfb.config.ts
   import { defineConfig } from "zfb/config";

   export default defineConfig({
     markdown: {
       gfm: {
         strikethrough: true,
         table: true,
         autolinkLiteral: false,    // explicit opt-out
         taskListItem: false,
         footnoteDefinition: false,
       },
     },
   });
   ```

3. **Omitted entirely** — the parser uses the conservative default.
   `~~text~~` parses as `<del>text</del>` and pipe tables render as
   `<table>`; every other GFM construct stays off.

   ```ts
   export default defineConfig({
     // no `markdown` field — strikethrough + table on, everything else off
   });
   ```

The five constructs you can toggle are: `strikethrough`, `table`,
`autolinkLiteral`, `taskListItem`, `footnoteDefinition`.

Projects that previously relied on raw `~~text~~` passing through as
literal characters should set `markdown: { gfm: { strikethrough: false } }`
or `markdown: { gfm: false }` to restore the old behaviour.

## Tests

```sh
pnpm --filter @takazudo/zfb test
```

The tests run under [vitest][vitest] with [happy-dom][happy-dom] as the
DOM implementation; no real browser is required.

[vitest]: https://vitest.dev/
[happy-dom]: https://github.com/capricorn86/happy-dom
