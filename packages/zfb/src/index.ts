// Public entry point for the "@takazudo/zfb" package.
//
// User TSX pages reach this module via `import { Island } from "@takazudo/zfb"`.
// The hydration runtime (Sub 3) reaches the helper via
// `import { scheduleHydrate } from "@takazudo/zfb/runtime"` (or by inlining the
// same logic; coordinated separately).

export {
  ANONYMOUS_COMPONENT_NAME,
  HYDRATE_MARKER_ATTR,
  Island,
  SKIP_SSR_MARKER_ATTR,
  resolveWhen,
  type IslandElement,
  type IslandProps,
} from "./island.js";
export {
  scheduleHydrate,
  mountIslands,
  mountNewIslands,
  cancelPendingIslands,
  unmountIslands,
} from "./runtime.js";
export type { IslandManifest, IslandManifestValue } from "./runtime.js";
export { DEFAULT_WHEN, isWhen, WHEN_VALUES, type When } from "./types.js";

// `defaultComponents` (htmlOverrides convention) is re-exported from the
// root entry point so `import { defaultComponents } from "@takazudo/zfb"` is the
// canonical access path. Each named override is also re-exported so
// consumers can tree-shake-import a single one (`import { ContentLink } from "@takazudo/zfb"`)
// without dragging in the whole map.
// Plugin lifecycle types + `definePlugin` identity helper. Plugin
// authors typically import these from "@takazudo/zfb/plugins" but the
// root entry re-exports them so simple plugins can pull everything from
// one path.
export {
  definePlugin,
  type ZfbBuildHookContext,
  type ZfbDevMiddlewareContext,
  type ZfbDevMiddlewareHandler,
  type ZfbDevMiddlewareRequest,
  type ZfbDevMiddlewareResponse,
  type ZfbPlugin,
  type ZfbPluginLogger,
} from "./plugins.js";

export {
  ContentBlockquote,
  ContentCode,
  ContentH2,
  ContentH3,
  ContentH4,
  ContentLink,
  ContentOl,
  ContentParagraph,
  ContentStrong,
  ContentTable,
  ContentUl,
  defaultComponents,
  type ContentComponentElement,
  type ContentComponentProps,
} from "./content.js";
