# zfb-render runtime assets

This directory holds JS/TS assets that the `zfb-render` Rust crate ships
alongside its compiled binary. The runtime loader (Sub 3) registers these
modules so user TSX pages can import them as bare specifiers.

## Files

- `zfb-sdk.js` — the canonical SDK source. The runtime resolves the bare
  module specifier `"zfb"` to this file, so user pages can write
  `import { paginate } from "zfb";`.
- `zfb-sdk.d.ts` — TypeScript declarations for the same module surface.
- `__tests__/zfb-sdk.test.mjs` — unit tests, runnable with Node's built-in
  test runner (Node 20+).

## Public SDK surface

Exported from `zfb-sdk.js`:

- `paginate({ items, pageSize })` — split an array into `paths()`-shaped
  page objects of the form
  `{ params: { page: "<n>" }, props: { current, total, pageSize, items } }`.
  Always returns at least one page (an empty input array yields a single
  empty page so route generation still produces a `page=1` slug).
- `__zfbVersion` — version string for runtime introspection. Pre-1.0 and
  subject to change.

## Running the tests

The tests deliberately avoid third-party dependencies so they can run
before any of the Rust crates have been scaffolded:

```sh
node --test crates/zfb-render/runtime/__tests__/zfb-sdk.test.mjs
```

Use Node 20+ (the project's engines pin a newer Node toolchain). Per
ADR-007 the production execution path runs the SDK through the embedded
V8 host driven by `@takazudo/zfb-runtime`; the Node tests here stay as
the fast feedback loop for SDK-only changes and do not exercise the host
boundary.

## Coordination notes

- The `zfb-render` Cargo crate is owned by Sub 3 and is not yet
  scaffolded in this branch. The runtime files live under
  `crates/zfb-render/runtime/` so they merge cleanly when Sub 3's crate
  skeleton lands.
- Do not add npm/npx dependencies for these tests — the project is
  pnpm-only and these tests are intentionally zero-dependency.
