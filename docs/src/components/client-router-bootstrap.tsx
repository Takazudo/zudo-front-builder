"use client";

// Client-side bootstrap for the @takazudo/zfb-runtime ClientRouter.
//
// Why this exists (zudolab/zudo-doc#1524, W7A verification):
// `<ClientRouter />` (mounted in the doc layout) emits SSR meta tags + CSS
// for the SPA soft-swap router, but the actual click/form intercept
// registration runs as a top-of-module side effect inside
// `@takazudo/zfb-runtime/src/client-router.ts`:
//
//   if (typeof document !== "undefined") { init(); }
//
// The host's `import { ClientRouter } from "@takazudo/zfb-runtime"` happens
// during SSR (where typeof document === "undefined"), so the side effect is
// silently skipped — the router module never reaches the client bundle and
// every navigation falls through to a full page load. (Confirmed on the
// Cloudflare deploy: the islands bundle shipped zero client-router code, so
// internal links did full reloads instead of SPA transitions — issue #883.)
//
// The fix is a minimal "use client" island whose only job is to
// side-effect-import `@takazudo/zfb-runtime/client-router` so the same guard
// fires in the browser. `init()` is idempotent (guarded by an `initialized`
// flag in router.ts), so reaching the import again later registers once.
//
// Bundle cost: only the client-router subgraph (router.ts + swap-functions +
// events + types + cssesc) plus its island-manager dep. The server-only
// zfb-runtime modules stay out of the bundle (not transitively reachable
// from the client-router barrel).
//
// Why a side-effect import, not init() in useEffect: module top-level code
// runs as soon as the browser parses the bundle — earlier than Preact mount,
// so the router intercepts the first click on a fresh page.

// Side-effect import — running this file's bundle in the browser triggers the
// `if (typeof document !== "undefined") { init(); }` guard at the top of
// `@takazudo/zfb-runtime/src/client-router.ts`. This is the line the
// scaffold's no-op stub omits; replacing the stub with this file is how a
// consumer opts the SPA router (and its view-transition animations) in.
import "@takazudo/zfb-runtime/client-router";

import type { JSX } from "preact";

/**
 * Renders nothing. The island marker exists only so zfb's island scanner
 * walks page -> BodyEndIslands -> ClientRouterBootstrap and includes the
 * client-router barrel in the per-island bundle, where the side-effect
 * import above can fire on the client.
 */
function ClientRouterBootstrap(): JSX.Element | null {
  return null;
}

// Stable marker name across SSR / scanner / hydration manifest. Required for
// production minification per the pattern in `pages/lib/_body-end-islands.tsx`.
ClientRouterBootstrap.displayName = "ClientRouterBootstrap";

export default ClientRouterBootstrap;
