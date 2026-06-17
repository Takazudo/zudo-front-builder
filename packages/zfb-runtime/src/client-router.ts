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
// Framework-agnostic element minting (no JSX syntax):
//   `@takazudo/zfb-runtime` does not depend on a framework runtime. Head nodes
//   are minted by calling `jsx` from `react/jsx-runtime` directly — NOT JSX
//   syntax, so this stays a plain `.ts` file with no tsconfig JSX changes. The
//   engine alias-rewrites `react/jsx-runtime` → `preact/jsx-runtime` in Preact
//   mode (bundler.rs ~2886) and resolves it natively in React mode, so the same
//   call mints a real element for whichever framework the project configured.
//   The previous approach (a hand-rolled `{ type, props, key, constructor:
//   undefined }` object literal — the Preact diff-path sentinel) only worked for
//   Preact: React's renderer rejects such an object as a child with React error
//   #31 ("Objects are not valid as a React child"), because a real React element
//   carries `$$typeof: Symbol.for("react.element")` a literal cannot fake. Same
//   migration as `Island` in @takazudo/zfb.
//
// The component renders three base sibling elements to <head> (plus optional
// metas emitted conditionally — see `prefetchDisabled` and `preserveHtmlAttrs`):
//   1. A <style> tag with the `.zfb-route-announcer` ARIA helper class.
//   2. <meta name="zfb-view-transitions-enabled" content="true" />
//   3. <meta name="zfb-view-transitions-fallback" content={fallback} />
//
// The route-announcer <div> is injected into <body> by `announce()` in
// `client-router/router.ts` on every navigation.

import { jsx } from "react/jsx-runtime";

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
  /**
   * Extra `<html>` attribute names to preserve across SPA swaps. By default the
   * client router copies the incoming server-rendered document's `<html>`
   * attributes onto the live root, dropping any current attribute that isn't
   * internal to the transition machinery — so a *runtime* attribute a consumer
   * sets from a persisted island (e.g. `data-theme` or `data-sidebar-hidden`
   * driven from `localStorage`) is lost on every navigation. List those names
   * here and the router re-applies their current value after each swap. Emitted
   * as a `<meta name="zfb-preserve-html-attrs">` tag that `swapRootAttributes`
   * reads at swap time.
   * @see https://github.com/Takazudo/zudo-front-builder/issues/1103
   */
  preserveHtmlAttrs?: string[];
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
 * Mint a head element through the per-project JSX runtime.
 *
 * Calls `jsx` from `react/jsx-runtime` (alias-rewritten to
 * `preact/jsx-runtime` in Preact mode by the engine, native in React mode)
 * so the returned value is a real element for whichever framework the
 * project configured — NOT a hand-rolled `{ type, props, key }` literal,
 * which only Preact accepts and which makes React throw error #31. A stable
 * `key` is passed because `ClientRouter()` returns these nodes in a plain
 * array (React warns about keyless list children otherwise). Mirrors the
 * `Island` migration in `@takazudo/zfb`.
 */
function makeVNode(type: string, props: Record<string, unknown>, key: string): ClientRouterElement {
  // `jsx`'s `type` param is typed `ElementType` (string-literal intrinsic
  // tags or component types), which rejects an arbitrary runtime `string`.
  // The tag is dynamic here, so cast to the factory's own first-param type —
  // robust whether the engine aliases `jsx` to react or preact at build time.
  return jsx(type as Parameters<typeof jsx>[0], props, key) as unknown as ClientRouterElement;
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
  preserveHtmlAttrs = [],
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
    makeVNode("style", { dangerouslySetInnerHTML: { __html: announcerCss } }, "zfb-vt-style"),
    // Opt-in meta tag: router checks for this to decide whether to intercept navigations.
    makeVNode("meta", { name: "zfb-view-transitions-enabled", content: "true" }, "zfb-vt-enabled"),
    // Fallback strategy meta tag: read by getFallback() in router.ts.
    makeVNode(
      "meta",
      { name: "zfb-view-transitions-fallback", content: fallback },
      "zfb-vt-fallback",
    ),
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
    nodes.push(
      makeVNode(
        "meta",
        { name: "zfb-prefetch-disabled", content: "true" },
        "zfb-prefetch-disabled",
      ),
    );
  }

  // Consumer-extensible <html> attribute preserve-list (#1103). swapRootAttributes
  // reads this meta and re-applies the listed attributes' runtime values after each
  // swap, so persisted-island state on <html> (data-theme, data-sidebar-hidden, …)
  // survives navigation. Emitted only when non-empty, so non-opt-in output stays
  // byte-identical. Same conditional-emission shape as the prefetch-disabled meta.
  const preserveList = preserveHtmlAttrs.filter(Boolean);
  if (preserveList.length > 0) {
    nodes.push(
      makeVNode(
        "meta",
        { name: "zfb-preserve-html-attrs", content: preserveList.join(" ") },
        "zfb-preserve-html-attrs",
      ),
    );
  }

  return nodes;
}
