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

export type { Direction, Fallback, NavigationTypeString, Options } from "./types.js";

export { swapFunctions, swap } from "./swap-functions.js";

// Router public surface.
//   - W3C1: `supportsViewTransitions`, `transitionEnabledOnThisPage`.
//   - W3C2: `navigate()` (public navigation entry).
//   - W3C3: `init()` (idempotent bootstrap: registers click + form intercept listeners).
export { navigate, supportsViewTransitions, transitionEnabledOnThisPage, init } from "./router.js";
export type { InitOptions } from "./router.js";

// (cssesc is an internal helper, not part of the public surface — not re-exported.)
