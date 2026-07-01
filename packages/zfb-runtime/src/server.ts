// Server-only entry point for `@takazudo/zfb-runtime` (subpath `/server`).
//
// `createPageRouter` builds a Hono app, so importing it pulls `hono` into the
// module graph. That is fine — even *desirable* — on the SSR / Worker side,
// but it must never reach a `--platform=browser` island bundle. Keeping the
// factory behind this dedicated subpath (rather than the client-safe `.`
// barrel in `./index.ts`) is what lets esbuild bundle an island that imports
// the bare `@takazudo/zfb-runtime` root without ever needing to resolve
// `hono` (issue #1298).
//
// The zfb SSR emit (`crates/zfb-build/src/bundler.rs`) imports
// `createPageRouter` from here.

export { createPageRouter } from "./router.js";
export type {
  CreatePageRouterOptions,
  PageDefinition,
  PageHeading,
  PageModule,
  PageRouter,
} from "./router.js";
