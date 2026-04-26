# zfb

Public SDK module for [zudo-front-builder][zfb-repo]. User pages reach this
package through the bare specifier `"zfb"` — the `zfb-render` runtime
loader registers the source under that name at build time so user TSX can
write:

```tsx
import { Island } from "zfb";
```

[zfb-repo]: https://github.com/Takazudo/zudo-front-builder

## What lives here

This package is the canonical TypeScript source for the `zfb` SDK
surface. Today it covers:

- `<Island when="visible|idle|load">` — JSX wrapper that marks a region
  for client-side hydration.
- `scheduleHydrate(target, when, fire)` — the runtime branching helper
  consumed by the hydration runtime (Sub 3).
- `When`, `WHEN_VALUES`, `DEFAULT_WHEN`, `isWhen`, `resolveWhen` — type
  and runtime utilities pinning the spelling of the three modes.

The package is JSX-runtime-agnostic: the `Island` component does not
import preact or react, so it works under either framework adapter
without bundling the wrong runtime.

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
    </>
  );
}
```

## The three `when=` modes

| `when`      | Trigger                                                         | Fallback                                       |
| ----------- | --------------------------------------------------------------- | ---------------------------------------------- |
| `"load"`    | Synchronous, immediate fire after registration. **Default.**    | n/a                                            |
| `"idle"`    | `requestIdleCallback`                                           | `setTimeout(0)` when not available             |
| `"visible"` | `IntersectionObserver`, threshold 0, first intersection only    | Immediate fire when `IntersectionObserver` is missing |

Unknown values produce a `console.warn` in development builds and fall
back to `"load"`.

## Build-time output

The wrapper is intentionally type-erased at the JSX boundary. At the
call site, `<Island when="visible">{children}</Island>` renders as:

```html
<div data-zfb-island data-when="visible"><!-- children --></div>
```

The `data-zfb-island` attribute is empty here. The hydration emit step
(`zfb-render` runtime, Sub 3) walks rendered HTML and replaces it with
`data-zfb-island="ComponentName"` so the client-side hydration runtime
can look up the right module to call.

## Runtime helper

The hydration runtime imports (or inlines) `scheduleHydrate` from this
package:

```ts
import { scheduleHydrate } from "zfb/runtime";

for (const el of document.querySelectorAll<HTMLElement>("[data-zfb-island]")) {
  const when = el.getAttribute("data-when") ?? "load";
  scheduleHydrate(el, when, () => hydrateOne(el));
}
```

`scheduleHydrate` returns a `cancel` function that aborts the schedule
if hydration has not fired yet. After firing, calling `cancel` is a
no-op.

## Tests

```sh
pnpm --filter zfb test
```

The tests run under [vitest][vitest] with [happy-dom][happy-dom] as the
DOM implementation; no real browser is required.

[vitest]: https://vitest.dev/
[happy-dom]: https://github.com/capricorn86/happy-dom
