// Lightweight ambient JSX types so the public Island wrapper does not
// hard-depend on either preact or react as a runtime/types dependency.
//
// BCI-4: `IslandProps.children` is now typed as `VNode` (a structural union)
// rather than the old `ReactNode` alias, so non-React frameworks can
// implement `IslandProps` without any React type dependency at the type level.
//
// Both Preact and React's `children` accept this structural union; the build
// is type-erased so the actual rendering happens in the user's app under
// whichever framework adapter they chose.

/**
 * Structural JSX element (VNode). Framework-agnostic: both Preact and React
 * produce objects that satisfy this shape when a JSX element is rendered.
 *
 * The `props` bag may itself contain `children` (nested VNodes) as well as
 * HTML attribute values (strings, numbers, booleans). All fields are widened
 * to `unknown` so the type stays opaque to callers that don't know which
 * framework produced the value.
 *
 * `type` accepts BOTH function components (call signature) AND class
 * components (construct signature). Preact and React both expose
 * `ComponentType = FunctionComponent | ComponentClass`, and the JSX runtime
 * stores either on `.type`. Without the construct signature, valid Preact
 * `<MyComponent />` expressions fail to assign to `VNode` at the framework
 * boundary (e.g. inside `<Island>` children) — even though class components
 * are rare in modern Preact, the `ComponentType` union includes them.
 */
export type VNodeObject = {
  readonly type: string | ((...args: unknown[]) => unknown) | (new (...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * A renderable JSX node: a primitive, `null`, `undefined`, an array, a
 * structured VNode object, or a bare `object`.
 *
 * This union covers every value both Preact and React treat as valid
 * `children`. The bare `object` member is required to make Preact's
 * `ComponentChildren` / `VNode<{}>` assignable without casts — Preact's own
 * `ComponentChild` uses the same widening pattern. The loosening is safe
 * because all SDK runtime code duck-types `unknown` at the boundary
 * (island.ts, content.ts) and the Rust pipeline is type-erased.
 *
 * Importantly, the union does **not** reference any framework-specific type,
 * so packages that depend on `zfb` need not install a React / Preact `@types`
 * package to satisfy this type.
 *
 * **Name-collision note for Preact consumers:** if your file already has
 * `import { VNode } from "preact"`, use a qualifier on the zfb import:
 * `import type { VNode as ZfbVNode } from "@takazudo/zfb"`.
 */
export type VNode =
  | string
  | number
  | boolean
  | null
  | undefined
  | bigint
  | VNodeArray
  | VNodeObject
  | object;

export interface VNodeArray extends ReadonlyArray<VNode> {}

// Back-compat alias — `ReactNode` was the previous name; code inside this
// package that has not been migrated yet can still refer to it. New code
// should use `VNode` directly.
/** @deprecated Use `VNode` instead. */
export type ReactNode = VNode;
