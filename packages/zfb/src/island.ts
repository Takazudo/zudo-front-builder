// Build-time `<Island when="...">` JSX wrapper.
//
// The wrapper is intentionally JSX-runtime-agnostic: it does not import
// preact or react and never calls h() / createElement directly. Instead it
// returns a plain object with a fixed shape that both Preact's and React's
// jsx-runtime accept when the JSX transform turns the call site into a
// jsx(Island, props) invocation.
//
// At build time, running through the SSR renderer (embedded V8 host):
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

import type { VNode } from "./jsx-types.js";
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

/**
 * Attribute the SSR wrapper writes to ferry the wrapped component's props
 * across the SSR → hydrate boundary. The hydration runtime parses this
 * with `JSON.parse` and forwards the result to the per-island `mount()`
 * call so the hydrated component sees the same props the SSR pass did.
 *
 * Omitted entirely when the wrapped child has no own data props (other
 * than `children`) — `readProps` already falls back to `{}` when the
 * attribute is missing, and emitting `data-props=""` would just bloat
 * the SSR markup.
 */
export const PROPS_DATA_ATTR = "data-props";

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
  ssrFallback?: VNode;
  /**
   * Server-rendered children, hydrated client-side once `when` fires.
   *
   * Typed as `VNode` (structural union) rather than `ReactNode` so
   * non-React frameworks can implement `IslandProps` without a React
   * type dependency (BCI-4).
   */
  children?: VNode;
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
  // Always source props from `props.children` (the heavy component VNode),
  // never from `ssrFallback` — the fallback is just SSR placeholder markup;
  // the hydrated component is the child, so its props are what `mount()`
  // needs at hydrate time. (Same rationale as `captureComponentName`.)
  const dataProps = captureSerializableProps(props.children);

  if (isSkipSsr) {
    const skipSsrProps: Record<string, unknown> = {
      [SKIP_SSR_MARKER_ATTR]: componentName,
      "data-when": when,
      children: props.ssrFallback ?? null,
    };
    if (dataProps !== undefined) skipSsrProps[PROPS_DATA_ATTR] = dataProps;
    return makeVNode({
      type: "div",
      props: skipSsrProps,
      key: null,
    });
  }

  const hydrateProps: Record<string, unknown> = {
    [HYDRATE_MARKER_ATTR]: componentName,
    "data-when": when,
    children: props.children,
  };
  if (dataProps !== undefined) hydrateProps[PROPS_DATA_ATTR] = dataProps;
  return makeVNode({
    type: "div",
    props: hydrateProps,
    key: null,
  });
}

/**
 * Construct the structural JSX-element shape with `constructor` set to
 * `undefined`.
 *
 * Why: `preact-render-to-string` (and Preact's diff path) recognise a
 * VNode by checking `vnode.constructor === undefined`. A plain object
 * literal `{ type, props, key }` inherits `Object` for `.constructor`,
 * so the renderer treats it as foreign data and emits no markup —
 * which would silently strip every `<Island>` from SSG output. Marking
 * `constructor: undefined` is the documented sentinel that both
 * Preact's and React's renderers treat as a structural VNode.
 *
 * The cast back to `IslandElement` keeps the public type tight; the
 * extra runtime field is JS-only and is invisible to type consumers.
 */
function makeVNode(shape: {
  type: string;
  props: Record<string, unknown>;
  key: unknown;
}): IslandElement {
  const v = shape as unknown as { constructor?: unknown };
  v.constructor = undefined;
  return shape as IslandElement;
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

/**
 * Serialize the wrapped child's data props as a JSON string the runtime
 * can parse out of the `data-props` attribute on the Island marker div.
 *
 * Mirrors [`captureComponentName`]'s array handling: when `children` is
 * an array (multiple JSX siblings), the first child whose own props
 * yield a non-empty serialization wins. This keeps the "first
 * identifiable child" contract consistent across both attributes —
 * whatever the marker name points at is what the data-props payload
 * describes.
 *
 * Returns `undefined` (not `"{}"` and not `""`) when no usable props
 * exist. The runtime's `readProps` already maps a missing attribute to
 * `{}`, so omitting the attribute keeps the SSR markup smaller and
 * preserves the invariant that the attribute, when present, always
 * parses to a non-empty record.
 *
 * Exported for tests; not re-exported from `index.ts`.
 */
export function captureSerializableProps(children: unknown): string | undefined {
  if (Array.isArray(children)) {
    for (const child of children) {
      const json = propsFromSingle(child);
      if (json !== undefined) return json;
    }
    return undefined;
  }
  return propsFromSingle(children);
}

function propsFromSingle(child: unknown): string | undefined {
  if (!child || typeof child !== "object") return undefined;
  const c = child as { props?: unknown };
  const raw = c.props;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return undefined;

  // Exclude `children` from the serialized payload: the SSR pass already
  // emitted the rendered children into the DOM (the hydration target),
  // and JSX child nodes are typically VNode trees with non-serializable
  // shapes (functions, circular refs) that would either bloat the
  // payload or throw inside JSON.stringify. The hydration runtime
  // re-renders into the existing DOM, so it does not need the JSX
  // children replayed through props.
  //
  // Note: JSON.stringify already silently drops function / symbol /
  // undefined values from the output, so those don't need explicit
  // pre-filtering here.
  const propsRecord = raw as Record<string, unknown>;
  let hasOwn = false;
  const filtered: Record<string, unknown> = {};
  for (const key of Object.keys(propsRecord)) {
    if (key === "children") continue;
    filtered[key] = propsRecord[key];
    hasOwn = true;
  }
  if (!hasOwn) return undefined;

  let json: string | undefined;
  try {
    json = JSON.stringify(filtered);
  } catch {
    // Circular references, BigInt, or any other non-serializable input
    // — silently fall through (no `data-props`) rather than ship a
    // partial payload. The runtime already handles the missing-attribute
    // case by returning `{}` from `readProps`.
    return undefined;
  }

  // JSON.stringify can also return `undefined` (when the top-level value
  // serializes to nothing) or `"{}"` (when every key was a function /
  // symbol / undefined and got dropped). Treat both as "nothing useful
  // to ship" so the marker stays clean.
  if (json === undefined || json === "{}") return undefined;
  return json;
}
