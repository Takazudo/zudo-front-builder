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
// The README documents the bundle shape T6 (miniflare orchestration)
// consumes.

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
