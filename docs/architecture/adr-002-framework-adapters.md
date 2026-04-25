# ADR-002: Framework adapter portable-component contract

- **Status:** Accepted
- **Date:** 2026-04-26
- **Deciders:** zfb core team
- **Related:** ADR-001 (JS runtime selection), Epic #4 (file-based router + JSX rendering)

## Context

zfb's rendering pipeline must support two server-side JSX frameworks:

- **Preact** (default) — small, fast, minimal API surface, ships with `preact-render-to-string` for synchronous SSR.
- **React** — first-class because the broader component ecosystem targets React, and many users want to reuse existing components or libraries.

Both options share a JSX surface, both support an automatic JSX runtime in modern transformers, and both expose a synchronous `renderToString` entry that fits zfb's static-site output model. Despite the family resemblance, they diverge in subtle, real ways:

- Different module specifiers for the JSX runtime (`preact/jsx-runtime` vs `react/jsx-runtime`).
- Different render-to-string modules (`preact-render-to-string` vs `react-dom/server`).
- React 18+ split rendering surfaces (`renderToString`, `renderToReadableStream`, `renderToPipeableStream`); Preact has a single sync entry.
- Different hook ecosystems (Preact's `useResource`, signals; React's `useTransition`, `useDeferredValue`, etc.).
- Different attribute and event normalization on the client at hydration time.

We need a story that lets a user pick their framework once, in `zfb.config.ts`, and have the build pipeline swap implementations cleanly — without the user authoring per-file pragmas, and without zfb attempting to abstract away framework differences in a leaky compatibility shim.

## Decision

**Two adapters, config-time selection.**

1. `zfb.config.ts` exposes a `framework: "preact" | "react"` field. Default is `"preact"`. The choice is read once at config-load time and threaded through the build.

2. `crates/zfb-render` ships two adapter implementations under `src/adapters/`:
   - `PreactAdapter` (default).
   - `ReactAdapter`.

   Both implement a single, narrow `Adapter` trait whose entire surface is:

   - `name() -> &'static str`
   - `jsx_import_source() -> &'static str`
   - `render_to_string_module() -> &'static str`
   - `pre_render_setup(&self, host: &mut dyn RenderHost) -> Result<(), RenderError>`

3. JSX runtime selection is **centralized** through SWC's `transform-react` config (`runtime: "automatic"`, `importSource: <adapter>.jsx_import_source()`). We do **not** rely on per-file `@jsxImportSource` pragmas — pragmas are intrusive, easy to forget, and create drift between files in the same project.

4. Each adapter installs a tiny pre-render shim into `globalThis`:

   ```js
   import { renderToString } from "<framework-render-module>";
   globalThis.__zfbRenderToString = renderToString;
   ```

   The render orchestrator in `render.rs` then calls `__zfbRenderToString(vnode)` uniformly, with no branching on framework identity past this point.

5. The React adapter uses `react-dom/server`'s `renderToString` — the **synchronous** API. We deliberately do not use `renderToReadableStream` or `renderToPipeableStream`, because zfb produces ahead-of-time static HTML; a sync string is simpler, deterministic, and avoids dragging Node streaming primitives into the build host.

## Portable-component contract (normative)

If a user wants their component code to work unchanged under either framework — for example, a shared component library used across two zfb projects with different framework choices — they MUST follow these rules. zfb does **not** enforce them at build time. They are a documented convention.

### 1. Hooks

- Use only the standard hooks present in **both** framework cores: `useState`, `useEffect`, `useLayoutEffect`, `useRef`, `useMemo`, `useCallback`, `useContext`, `useReducer`.
- Do **not** import from framework-specific hook modules:
  - No `import { useResource } from "preact-iso"` or similar Preact-only hooks.
  - No `import { useTransition, useDeferredValue, useId, useSyncExternalStore } from "react"` (these are React-only or have subtly different semantics in Preact compatibility mode).

### 2. Signals and reactive primitives

- `@preact/signals` and `@preact/signals-react` are **NOT portable**. They look interoperable but the runtime mechanics (auto-tracking via observable proxies, integration with the renderer's reconciler) differ enough that a portable component cannot rely on them.
- Cross-framework portable components manage state through hooks only.

### 3. Event handlers

- Use the union of the two frameworks' supported event names. In practice this means: `onClick`, `onInput`, `onSubmit`, `onChange`, `onFocus`, `onBlur`, `onKeyDown`, `onKeyUp`. All of these work in both.
- For text inputs in **controlled** components, see "controlled inputs" below.

### 4. Attributes

- Use **React conventions** even when targeting Preact: `className`, `htmlFor`, lowercase-with-dashes for `aria-*` (e.g. `aria-label`, `aria-labelledby`). Preact accepts these. The reverse is not always true.
- Boolean attributes: pass `true` / `false`, not the string `"true"`.

### 5. Imports

- Import JSX-using components plainly. Do **not** import directly from `preact/hooks`, `preact/jsx-runtime`, `react/jsx-runtime`, or `react-dom/*`. Let the adapter and the SWC transform pick the right module.
- For hooks, prefer the "barrel" import from the framework root (`from "preact"` / `from "react"`); SWC's automatic runtime handles the JSX entry independently.

### 6. Hydration

- Portable components must not assume a specific hydration entry point. zfb owns the hydration call site and emits the right one (`hydrate` for Preact, `hydrateRoot` for React 18+) from the framework's client runtime.

## Gotchas (non-normative — context for ADR readers)

These are the *known* divergences. The list is not exhaustive; the build does not detect them.

### Preact-specific APIs are NOT portable

- Signals (`@preact/signals`, `@preact/signals-react`) are tightly coupled to the renderer they target. They are explicitly out of the portable contract.
- `useResource`, `useErrorBoundary`, and other Preact-only hooks must be replaced with portable equivalents (`useState` + `useEffect` for resource fetching, an explicit error-boundary class for error handling).
- React-only hooks (`useTransition`, `useDeferredValue`, `useId`, `useSyncExternalStore`) are likewise out of the portable contract.

### Automatic JSX runtime differs subtly

- React 18+ and Preact both expose an automatic JSX runtime that lets the transformer emit `import { jsx, jsxs } from "<framework>/jsx-runtime"` without the component author writing `import { h } from "preact"` or `import React from "react"`.
- SWC's `transform-react` with `runtime: "automatic"` + `importSource: "<framework>"` handles the import injection centrally.
- However: if user code (or a transitive dependency) explicitly imports from `preact/jsx-runtime` or `react/jsx-runtime`, the transform does not unify these. Manually-imported `jsx` symbols can leak between modules and produce confusing render-time errors. The portable contract forbids importing from `*/jsx-runtime` directly (see rule 5).

### Server output is identical for portable components — client behaviour can diverge

The HTML produced by `preact-render-to-string` and `react-dom/server`'s `renderToString` is byte-equal for components inside the portable contract. Snapshot equality on the server is a real testable property. But once code runs in the browser:

- **`aria-*` casing.** React lowercases ARIA attribute names on server output but normalizes them again at hydration. Preact passes them through verbatim. Mixed casing in source can survive on the server and break on the client. Mitigation: rule 4 above — write lowercase always.
- **Controlled inputs.** React strictly requires the `onChange` + `value` pair; Preact happily accepts `onInput` + `value`. A component written with `onInput` works in Preact but emits a "you provided a `value` prop without an `onChange` handler" warning under React. Mitigation: when both are needed, supply both handlers.
- **`hydrate` vs `hydrateRoot`.** Preact has a single `hydrate(vnode, container)`. React 18+ split this into `hydrateRoot(container, vnode)` with a different argument order and a returned root handle. The adapter, not the user, owns this call.

### Bundle-size trade-off

`react-dom/server` is roughly 50 KB larger than `preact-render-to-string` after gzip in the build host's bundled JS runtime image. This affects build-host memory and cold-start time, not end-user delivered HTML. Users who pick `framework: "react"` should know they are paying for parity with the React ecosystem, not for runtime performance.

## Enforcement

The portable-component contract is **documented convention only**. The build does not detect violations. A future linter integration (custom ESLint rules, a `zfb lint` subcommand) is **out of scope for v1** and explicitly deferred. Violations manifest as runtime errors when a project is rebuilt under the other framework — typically the moment a user tries to swap.

## Consequences

### Positive

- The user-facing surface is one config field. Swapping frameworks is a one-line change in `zfb.config.ts`.
- The adapter trait is small enough that adding a third framework later (Solid, Svelte SSR, etc.) is feasible — it requires a new adapter file, a new `Framework` enum variant, and a build-time renderer integration. The render orchestrator does not change.
- SWC handles JSX transform centrally; users do not write per-file pragmas.
- The render orchestrator in `render.rs` is framework-agnostic past the `__zfbRenderToString` shim install.

### Negative

- The portable contract is not statically enforced. Users who mix Preact-only or React-only APIs into shared components only find out at swap time.
- Two adapters means two sets of dependencies in the build host's JS module graph (`preact-render-to-string` for Preact, `react-dom/server` for React). Both are dragged in at build time even though only one is active per project.
- React 18+'s server entry has a wider API surface (streaming variants, suspense boundaries, server components). zfb intentionally uses only `renderToString` and inherits the limitations that come with that — for example, no streaming HTML, no React Server Components in v1.
- Subtle runtime differences between the frameworks (aria casing, controlled inputs, hook semantics) leak through when a portable component crosses a non-portable boundary. The contract documents the boundary; it does not eliminate the leak.

## Alternatives considered

### Per-file JSX pragmas

```ts
/** @jsxImportSource preact */
```

Rejected. Pragmas are intrusive, easy to forget, and put the framework choice in user files where it doesn't belong. They also create drift: a project mid-migration ends up with a mix of `preact` and `react` pragmas. Centralizing in `zfb.config.ts` + SWC config sidesteps this entirely.

### Unified abstraction over both libraries

A `zfb-jsx` package that re-exports a unified API (`useState`, `renderToString`, etc.) targeting both libraries. Rejected as a leaky abstraction: every divergence between the two frameworks (hooks, hydration, signals, server-side semantics) becomes a problem inside the abstraction layer, and inevitably the abstraction has to expose framework-specific escape hatches anyway. Better to pick one of two concrete frameworks and document the divergence than to invent a third.

### Only-Preact

Considered briefly. Rejected because React first-class support is a locked design decision for the project — many users have existing React components or React-only library dependencies they want to reuse. Cutting React would push these users off the platform.

### Single dynamic loader (runtime-detected framework)

Have the build try to import either `preact-render-to-string` or `react-dom/server` based on the user's `package.json` dependencies. Rejected because it makes the build's behaviour non-deterministic with respect to the user's `package.json` and obscures the "I am using framework X" signal. Explicit `framework:` config is clearer.
