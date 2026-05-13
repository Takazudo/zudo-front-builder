// `@takazudo/zfb-runtime` — `<ClientRouter />` component.
//
// Ported from Astro's `ClientRouter.astro` (155 lines).
// Source: packages/astro/components/ClientRouter.astro
// Issue: zudolab/zudo-doc#1519 (W3D), parent epic zudolab/zudo-doc#1510.
//
// Named-cause deviation — inline script split (W1B §13.6):
//   Astro's <script> block is processed by Vite at build time as a bundled
//   module with virtual-module imports (`astro:transitions/client`). The zfb
//   port emits meta tags + global styles from the JSX component, while the
//   click/form intercepts live in `client-router/router.ts` and are registered
//   via `init()`. The component calls `init()` as a side effect on first import.
//   No inline <script> dangerouslySetInnerHTML is emitted.
//
// Named-cause deviation — no JSX (W1B: "follow existing component-emission pattern"):
//   `@takazudo/zfb-runtime` does not depend on Preact and has no JSX compiler
//   options configured in its tsconfig. The existing component-emission pattern
//   (established by `Island` in @takazudo/zfb, and `ViewTransitions` in this
//   package) uses plain VNode-shaped objects with `constructor: undefined` — the
//   sentinel recognised by both Preact and React renderers. Following that pattern
//   avoids adding Preact as a dependency and avoids tsconfig JSX changes.
//
// The component renders three sibling elements to <head>:
//   1. A <style> tag with the `.zfb-route-announcer` ARIA helper class.
//   2. <meta name="zfb-view-transitions-enabled" content="true" />
//   3. <meta name="zfb-view-transitions-fallback" content={fallback} />
//
// The route-announcer <div> is injected into <body> by `announce()` in
// `client-router/router.ts` on every navigation.

import { init } from "./client-router/router.js";
import { init as prefetchInit } from "./client-router/prefetch.js";

// Side-effect: wire click + submit intercepts on first import of this component.
// Guarded by the idempotent `initialized` flag in router.ts — safe for multiple
// <ClientRouter /> mounts and HMR re-runs. (W3C3 init idempotency.)
if (typeof document !== "undefined") {
  init();
}

// Module-level guard: prefetch bootstrap is triggered at most once per module
// lifetime even if <ClientRouter prefetchAll /> is mounted multiple times (#276).
let prefetchBootstrapped = false;

export interface ClientRouterProps {
  /** Fallback animation strategy when native View Transitions are not supported. */
  fallback?: "none" | "animate" | "swap";
  /**
   * When true, opts every same-origin link into the default prefetch strategy
   * (hover) by calling prefetchInit({ prefetchAll: true }) once on the client.
   */
  prefetchAll?: boolean;
}

/**
 * Public element shape for each node returned by `<ClientRouter />`.
 * Structural type — intentionally matches the Preact/React VNode object shape
 * so consumers do not type-infer through the internal representation.
 */
export type ClientRouterElement = {
  readonly type: string;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * Construct a VNode-shaped object. Sets `constructor: undefined` so that both
 * Preact's and React's renderers treat it as a structural VNode rather than
 * foreign data. (Mirrors the `makeVNode` pattern in `@takazudo/zfb`'s Island.)
 */
function makeVNode(type: string, props: Record<string, unknown>): ClientRouterElement {
  const v = { type, props, key: null } as {
    type: string;
    props: Record<string, unknown>;
    key: null;
    constructor?: unknown;
  };
  v.constructor = undefined;
  return v as ClientRouterElement;
}

// CSS for the route-announcer element. Ported verbatim from Astro's
// `<style is:global>` block in ClientRouter.astro (lines 11–23), renaming
// `.astro-route-announcer` → `.zfb-route-announcer` per W1B §5.
// This is a global (non-scoped) <style> because the announcer <div> is
// appended to document.body at runtime, outside any Preact-controlled subtree.
const announcerCss = `
.zfb-route-announcer {
	position: absolute;
	left: 0;
	top: 0;
	clip: rect(0 0 0 0);
	clip-path: inset(50%);
	overflow: hidden;
	white-space: nowrap;
	width: 1px;
	height: 1px;
}
`;

/**
 * `<ClientRouter />` — SPA soft-swap navigation with View Transition animations.
 *
 * Mount once in your page `<head>`. Emits the opt-in meta tags and the global
 * `.zfb-route-announcer` stylesheet that the route-announcer ARIA div needs.
 * Click and form-submit intercepts are registered as a side effect of importing
 * this component (idempotent — safe to mount multiple times).
 *
 * @example
 * ```tsx
 * import { ClientRouter } from "@takazudo/zfb-runtime";
 * // In your page <head>:
 * <ClientRouter fallback="animate" />
 * ```
 */
export function ClientRouter({
  fallback = "animate",
  prefetchAll: prefetchAllProp = false,
}: ClientRouterProps = {}): readonly ClientRouterElement[] {
  // Bootstrap prefetch exactly once on the client when prefetchAll is true.
  // The initialized flag inside prefetchInit() provides a second safety layer
  // in case of concurrent hydration or manual callers (#276).
  if (typeof document !== "undefined" && prefetchAllProp && !prefetchBootstrapped) {
    prefetchBootstrapped = true;
    prefetchInit({ prefetchAll: true });
  }

  const nodes: ClientRouterElement[] = [
    // Global styles for the ARIA route-announcer div injected into <body>.
    makeVNode("style", { dangerouslySetInnerHTML: { __html: announcerCss } }),
    // Opt-in meta tag: router checks for this to decide whether to intercept navigations.
    makeVNode("meta", { name: "zfb-view-transitions-enabled", content: "true" }),
    // Fallback strategy meta tag: read by getFallback() in router.ts.
    makeVNode("meta", { name: "zfb-view-transitions-fallback", content: fallback }),
  ];

  // Prefetch-disabled meta tag (#277): emitted when the bundler set
  // `globalThis.__zfb.prefetchDisabled = true` (from `zfb.config.ts`
  // `prefetch: { disabled: true }`). The sibling prefetch-core module reads
  // `document.querySelector('meta[name="zfb-prefetch-disabled"][content="true"]')`
  // at `init()` time and short-circuits if found.
  //
  // The flag is site-wide and static — set once at bundle-emit time, never
  // per-page. This meta tag appears on every page that mounts `<ClientRouter />`
  // or not at all.
  //
  // Pin the contract verbatim — the attribute names and content value are
  // shared with the sibling prefetch-core sub-issue (#276).
  if ((globalThis as { __zfb?: { prefetchDisabled?: boolean } }).__zfb?.prefetchDisabled === true) {
    nodes.push(makeVNode("meta", { name: "zfb-prefetch-disabled", content: "true" }));
  }

  return nodes;
}
