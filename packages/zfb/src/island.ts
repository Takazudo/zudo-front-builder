// Build-time `<Island when="...">` JSX wrapper.
//
// The wrapper is intentionally JSX-runtime-agnostic: it does not import
// preact or react and never calls h() / createElement directly. Instead it
// returns a plain object with a fixed shape that both Preact's and React's
// jsx-runtime accept when the JSX transform turns the call site into a
// jsx(Island, props) invocation.
//
// At build time, running through the SSR renderer (miniflare host):
//
//   <Island when="visible"><Counter /></Island>
//
// renders as:
//
//   <div data-zfb-island="Counter" data-when="visible">
//     <Counter />
//   </div>
//
// The component-name attribute (`data-zfb-island="ComponentName"`) is filled
// in *here* by reading the child's JSX type identity (`displayName` first,
// then `name`). Sub 3's hydration shim then `querySelectorAll`s these
// markers and looks each one up in the islands manifest produced by the
// scanner (see `crates/zfb-islands/src/manifest.rs` for the contract).
//
// SSR-skip mode: when the caller passes `ssrFallback`, the heavy child is
// **not** evaluated at SSR time. Instead the wrapper emits a different
// marker:
//
//   <Island ssrFallback={<div>Loading…</div>}><HeavyClientOnly /></Island>
//   →
//   <div data-zfb-island-skip-ssr="HeavyClientOnly">
//     <div>Loading…</div>
//   </div>
//
// On the client the hydration runtime distinguishes hydrate vs. render by
// which marker attribute is present. This is the equivalent of Astro's
// `client:only="preact"`.
//
// When validation: in development we console.warn for unknown `when`
// values and fall back to the default. In production we silently fall
// back to keep the bundle path small.

import type { ReactNode } from "./jsx-types.js";
import { resolveWhen, type When } from "./types.js";

// Re-export `resolveWhen` for back-compat: tests and downstream consumers
// historically imported it from `./island.js`. The implementation lives in
// `./types.js` so the runtime scheduler can pull it in without dragging
// the JSX wrapper along for the ride.
export { resolveWhen } from "./types.js";

/**
 * Marker attribute the SSR wrapper writes when the child component should
 * be hydrated client-side. The hydration runtime queries
 * `[data-${HYDRATE_MARKER_ATTR}]` to find islands.
 */
export const HYDRATE_MARKER_ATTR = "data-zfb-island";

/**
 * Marker attribute the SSR wrapper writes when SSR is being skipped (the
 * client:only-equivalent path). The hydration runtime queries
 * `[data-${SKIP_SSR_MARKER_ATTR}]` to find these placeholders and renders
 * the real component into them on hydration — there is no server output
 * to patch up.
 */
export const SKIP_SSR_MARKER_ATTR = "data-zfb-island-skip-ssr";

/** Fallback name surfaced when child identity cannot be determined. */
export const ANONYMOUS_COMPONENT_NAME = "Anonymous";

/** Props for `<Island>`. */
export interface IslandProps {
  /** Hydration scheduling strategy. Defaults to `"load"`. */
  when?: When;
  /**
   * If supplied, switches the island into SSR-skip mode (Astro's
   * `client:only` equivalent). The wrapper emits the
   * `data-zfb-island-skip-ssr` marker, the heavy `children` are **not**
   * evaluated server-side, and `ssrFallback` is rendered in their place.
   * On hydration the client runtime swaps in the real component.
   */
  ssrFallback?: ReactNode;
  /** Server-rendered children, hydrated client-side once `when` fires. */
  children?: ReactNode;
}

/**
 * Public JSX-element shape returned by [`Island`]. Intentionally widened
 * to a structural type so consumers don't infer through the internal
 * `{ type, props, key }` VNode shape of either Preact or React. Both
 * jsx-runtimes accept this object on either side of the boundary.
 */
export type IslandElement = {
  readonly type: string;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * `<Island>` JSX wrapper.
 *
 * Returns a JSX element shape compatible with both Preact and React. The
 * runtime tag is `"div"`. In the default (hydrate) mode the wrapper emits
 * `data-zfb-island="ComponentName"` and `data-when="<resolved-when>"`. In
 * SSR-skip mode (when `ssrFallback` is provided) it emits
 * `data-zfb-island-skip-ssr="ComponentName"` instead and renders the
 * fallback rather than the heavy child.
 *
 * The component-name string is derived from the child JSX element's type
 * identity (`type.displayName ?? type.name`). For string-typed children
 * (host elements) the tag name is used. If no usable identity can be
 * recovered, [`ANONYMOUS_COMPONENT_NAME`] is used so the marker still
 * lines up with the hydration shim's manifest lookup.
 *
 * The return type is the public [`IslandElement`] shape — the internal
 * VNode structure is deliberately not leaked so consumers never type-infer
 * through it.
 */
export function Island(props: IslandProps): IslandElement {
  const when = resolveWhen(props.when);
  const componentName = captureComponentName(props.children);
  const isSkipSsr = props.ssrFallback !== undefined;

  if (isSkipSsr) {
    return {
      type: "div",
      props: {
        [SKIP_SSR_MARKER_ATTR]: componentName,
        "data-when": when,
        children: (props.ssrFallback ?? null) as ReactNode,
      },
      key: null,
    };
  }

  return {
    type: "div",
    props: {
      [HYDRATE_MARKER_ATTR]: componentName,
      "data-when": when,
      children: props.children as ReactNode,
    },
    key: null,
  };
}

/**
 * Pull a component-name string out of a JSX child.
 *
 * Both Preact and React store rendered VNodes as plain objects whose
 * `.type` field is either:
 * - the component function (look at `displayName ?? name`),
 * - or the host element tag name as a string.
 *
 * If `children` is an array (multiple children), the first child with a
 * usable identity wins. This is intentional: the typical island shape is
 * `<Island><Foo /></Island>` (single child); when the caller wraps a
 * fragment-like list we still want a deterministic, debuggable name.
 *
 * Exported for tests; not re-exported from `index.ts`.
 */
export function captureComponentName(children: unknown): string {
  if (Array.isArray(children)) {
    for (const child of children) {
      const name = nameFromSingle(child);
      if (name) return name;
    }
    return ANONYMOUS_COMPONENT_NAME;
  }
  return nameFromSingle(children) || ANONYMOUS_COMPONENT_NAME;
}

function nameFromSingle(child: unknown): string {
  if (!child || typeof child !== "object") return "";
  const c = child as { type?: unknown };
  const t = c.type;
  if (typeof t === "function") {
    const fn = t as { displayName?: unknown; name?: unknown };
    if (typeof fn.displayName === "string" && fn.displayName) return fn.displayName;
    if (typeof fn.name === "string" && fn.name) return fn.name;
    return "";
  }
  if (typeof t === "string" && t) return t;
  return "";
}
