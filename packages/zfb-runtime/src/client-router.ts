// `@takazudo/zfb-runtime` — `<ClientRouter />` activation shim.
//
// Component/activation split (#2437): the pure `<ClientRouter />` component
// (JSX, props, VNode minting — no side effects) lives in
// `./client-router-component.ts`. This module re-exports that surface and
// additionally runs `init()` from `client-router/router.ts` as a side effect
// on import, wiring the click/form-submit intercepts. Keeping this exact file
// path (rather than folding it away) preserves the package.json `sideEffects`
// entry, deep-import back-compat, and the subpath activation chain:
// `import "@takazudo/zfb-runtime/client-router"` (auto-injected by the
// islands bundler, `crates/zfb-islands/src/esbuild.rs`) resolves through
// `client-router/index.ts` to here, and evaluating this module activates the
// router byte-compatibly with pre-split behavior.
//
// The root barrel (`src/index.ts`) imports `ClientRouter` from
// `./client-router-component.js` instead, so `import { ClientRouter } from
// "@takazudo/zfb-runtime"` alone performs zero side effects.

import { init } from "./client-router/router.js";

export {
  ClientRouter,
  type ClientRouterProps,
  type ClientRouterElement,
} from "./client-router-component.js";

// Side-effect: wire click + submit intercepts on import of this shim.
// Guarded by the idempotent `initialized` flag in router.ts — safe for
// multiple imports and HMR re-runs. (W3C3 init idempotency.)
if (typeof document !== "undefined") {
  init();
}
