// Public API surface for the client-router module.
// This file is the barrel for @takazudo/zfb-runtime's client-router export.
// Subsequent W3 sub-issues will add re-exports of swap-functions, router, and the component.

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
} from "./events.js";

export type { Direction, Fallback, NavigationTypeString, Options } from "./types.js";

export { swapFunctions, swap } from "./swap-functions.js";

// W3C1 router-core public surface. `navigate()` is intentionally NOT re-exported
// here yet — W3C2 will add it once the public entry point is implemented.
export { supportsViewTransitions, transitionEnabledOnThisPage } from "./router.js";

// (cssesc is an internal helper, not part of the public surface — not re-exported.)
