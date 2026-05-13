// Public entry point for `@takazudo/zfb-runtime`.
//
// Worker bundles produced by T3 import this module and invoke
// `createPageRouter` once, at module top, to obtain the fetch handler:
//
//   import { createPageRouter } from "@takazudo/zfb-runtime";
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

export { createPageRouter } from "./router.js";
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
} from "./client-router/router.js";
export {
  TRANSITION_BEFORE_PREPARATION,
  TRANSITION_AFTER_PREPARATION,
  TRANSITION_BEFORE_SWAP,
  TRANSITION_AFTER_SWAP,
  TRANSITION_PAGE_LOAD,
  TransitionBeforePreparationEvent,
  TransitionBeforeSwapEvent,
  isTransitionBeforePreparationEvent,
  isTransitionBeforeSwapEvent,
} from "./client-router/events.js";
export { swapFunctions, swap } from "./client-router/swap-functions.js";
export type { Direction, Fallback, NavigationTypeString, Options } from "./client-router/types.js";

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
