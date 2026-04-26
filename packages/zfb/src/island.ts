// Build-time `<Island when="...">` JSX wrapper.
//
// The wrapper is intentionally JSX-runtime-agnostic: it does not import
// preact or react and never calls h() / createElement directly. Instead it
// returns a plain object with a fixed shape that both Preact's and React's
// jsx-runtime accept when the JSX transform turns the call site into a
// jsx(Island, props) invocation.
//
// At build time:
//   <Island when="visible">{children}</Island>
// renders as:
//   <div data-zfb-island data-when="visible">{children}</div>
//
// The component-name attribute (data-zfb-island="ComponentName") is filled
// in by the hydration emit step (Sub 3) when it walks rendered HTML and
// rewrites server output. This wrapper only sets the marker presence and
// data-when.
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

/** Props for `<Island>`. */
export interface IslandProps {
  /** Hydration scheduling strategy. Defaults to `"load"`. */
  when?: When;
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
 * runtime tag is `"div"` and the only props set on it are
 * `data-zfb-island` (always present, value empty until Sub 3 fills in
 * the component name) and `data-when` (the resolved scheduling strategy).
 *
 * The return type is the public [`IslandElement`] shape — the internal
 * VNode structure is deliberately not leaked so consumers never type-infer
 * through it.
 */
export function Island(props: IslandProps): IslandElement {
  const when = resolveWhen(props.when);
  return {
    type: "div",
    props: {
      "data-zfb-island": "",
      "data-when": when,
      children: props.children as ReactNode,
    },
    key: null,
  };
}
