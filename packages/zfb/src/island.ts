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
import { DEFAULT_WHEN, isWhen, type When } from "./types.js";

/** Props for `<Island>`. */
export interface IslandProps {
  /** Hydration scheduling strategy. Defaults to `"load"`. */
  when?: When;
  /** Server-rendered children, hydrated client-side once `when` fires. */
  children?: ReactNode;
}

/**
 * Resolve the validated `when` value, warning in development for unknown
 * inputs. Exported so tests can pin the warning behavior.
 */
export function resolveWhen(when: unknown): When {
  if (when === undefined) return DEFAULT_WHEN;
  if (isWhen(when)) return when;
  if (typeof process !== "undefined" && process.env && process.env["NODE_ENV"] !== "production") {
    // eslint-disable-next-line no-console
    console.warn(
      `[zfb] <Island when="${String(when)}"> is not a valid value. ` +
        `Expected "visible" | "idle" | "load". Falling back to "${DEFAULT_WHEN}".`,
    );
  }
  return DEFAULT_WHEN;
}

/**
 * `<Island>` JSX wrapper.
 *
 * Returns a JSX element shape compatible with both Preact and React. The
 * runtime tag is `"div"` and the only props set on it are
 * `data-zfb-island` (always present, value empty until Sub 3 fills in
 * the component name) and `data-when` (the resolved scheduling strategy).
 *
 * This function is intentionally untyped at the JSX-element-shape boundary:
 * Preact and React expose subtly different VNode shapes, but both renderers
 * accept the form `{ type, props, key }` produced by their jsx-runtime
 * transforms when called via `jsx(Island, props)`. Runtime test coverage
 * verifies the rendered DOM is correct for at least one renderer.
 */
export function Island(props: IslandProps): {
  type: "div";
  props: {
    "data-zfb-island": string;
    "data-when": When;
    children: ReactNode;
  };
  key: null;
} {
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
