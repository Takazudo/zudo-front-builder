// Public API surface for the client-router module.
// This file is the barrel for @takazudo/zfb-runtime's client-router export.
// W3D adds the <ClientRouter /> component and ClientRouterProps re-exports.

// Component (W3D).
export { ClientRouter, type ClientRouterProps } from "../client-router.js";

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
} from "./events.js";

export type {
  Direction,
  Fallback,
  NavigationTypeString,
  Options,
  SyncHistoryEntryOptions,
} from "./types.js";

export { swapFunctions, swap } from "./swap-functions.js";

// Router public surface.
//   - W3C1: `supportsViewTransitions`, `transitionEnabledOnThisPage`.
//   - W3C2: `navigate()` (public navigation entry).
//   - W3C3: `init()` (idempotent bootstrap: registers click + form intercept listeners).
//   - W2 (#1377): `syncHistoryEntry()` (history bookkeeping without navigation).
export {
  navigate,
  supportsViewTransitions,
  transitionEnabledOnThisPage,
  init,
  syncHistoryEntry,
} from "./router.js";
export type { InitOptions } from "./router.js";

// Prefetch public surface (#276).
// `init` from prefetch.ts is re-exported as `prefetchInit` to avoid colliding
// with the router's `init` above.
export { prefetch, init as prefetchInit } from "./prefetch.js";
export type { PrefetchStrategy, PrefetchInitOptions, PrefetchOptions } from "./prefetch.js";
