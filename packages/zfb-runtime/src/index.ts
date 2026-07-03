// Public entry point for `@takazudo/zfb-runtime`.
//
// This is the **client-safe** barrel: everything re-exported here must be
// bundleable for `--platform=browser` without pulling a server-only
// dependency. In particular `createPageRouter` (and the Hono server router
// it builds) is intentionally NOT re-exported here — it lives at the
// server-only subpath `@takazudo/zfb-runtime/server`. Re-exporting it from
// this barrel makes any island that does `import ... from "@takazudo/zfb-runtime"`
// drag `hono` into the browser graph, which esbuild must then resolve
// (issue #1298). The page-router *types* below are `export type` only, so
// esbuild strips them and no runtime edge to `./router.js` survives.
//
// Worker bundles produced by T3 import the server subpath and invoke
// `createPageRouter` once, at module top, to obtain the fetch handler:
//
//   import { createPageRouter } from "@takazudo/zfb-runtime/server";
//
//   const router = createPageRouter({
//     pages: [...],
//     contentSnapshot: __ZFB_CONTENT_SNAPSHOT__, // embedded by the bundler
//     framework: { renderToString: render },     // preact-render-to-string etc.
//   });
//
//   export default { fetch: router };
//
// The README documents the bundle shape T6 (embedded V8 host) consumes.

export type {
  CreatePageRouterOptions,
  PageDefinition,
  PageHeading,
  PageModule,
  PageRouter,
} from "./router.js";
export type { FrameworkAdapter } from "./framework.js";
export type { ContentSnapshot, EntrySnapshot } from "./snapshot.js";
export { ViewTransitions, type ViewTransitionsElement } from "./view-transitions.js";

// Client-router public surface (W3D — mirrors @takazudo/zfb-runtime/client-router barrel).
// See W1B §2 for the full public API spec.
export { ClientRouter, type ClientRouterProps } from "./client-router.js";
export {
  navigate,
  supportsViewTransitions,
  transitionEnabledOnThisPage,
  syncHistoryEntry,
} from "./client-router/router.js";

// Prefetch public surface (#276).
// `init` from prefetch.ts is re-exported as `prefetchInit` to avoid collision
// with the router's `init`.
export { prefetch, init as prefetchInit } from "./client-router/prefetch.js";
export type {
  PrefetchStrategy,
  PrefetchInitOptions,
  PrefetchOptions,
} from "./client-router/prefetch.js";
export {
  TRANSITION_BEFORE_PREPARATION,
  TRANSITION_AFTER_PREPARATION,
  TRANSITION_BEFORE_SWAP,
  TRANSITION_AFTER_SWAP,
  TRANSITION_PAGE_LOAD,
  TRANSITION_NAVIGATION_ABORTED,
  TransitionBeforePreparationEvent,
  TransitionBeforeSwapEvent,
  isTransitionBeforePreparationEvent,
  isTransitionBeforeSwapEvent,
} from "./client-router/events.js";
export { swapFunctions, swap } from "./client-router/swap-functions.js";
export type {
  Direction,
  Fallback,
  NavigationTypeString,
  Options,
  SyncHistoryEntryOptions,
} from "./client-router/types.js";

// Plugin lifecycle types (#255). The runtime package re-exports the
// `@takazudo/zfb/plugins` surface so consumers writing plugins from a
// project that only depends on `@takazudo/zfb-runtime` can still
// import the canonical types (e.g. `import type { ZfbPlugin } from
// "@takazudo/zfb-runtime"`).
export type {
  ZfbPlugin,
  ZfbPluginLogger,
  ZfbBuildHookContext,
  ZfbDevMiddlewareContext,
  ZfbDevMiddlewareHandler,
  ZfbDevMiddlewareRequest,
  ZfbDevMiddlewareResponse,
  ZfbRouteEntry,
  ZfbRouteManifest,
  ZfbSetupContext,
  ZfbVirtualModuleLoader,
} from "@takazudo/zfb/plugins";
