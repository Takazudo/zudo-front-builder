// Public entry point for the "zfb" package.
//
// User TSX pages reach this module via `import { Island } from "zfb"`.
// The hydration runtime (Sub 3) reaches the helper via
// `import { scheduleHydrate } from "zfb/runtime"` (or by inlining the
// same logic; coordinated separately).

export { Island, resolveWhen, type IslandProps } from "./island.js";
export { scheduleHydrate } from "./runtime.js";
export { DEFAULT_WHEN, isWhen, WHEN_VALUES, type When } from "./types.js";

// `defaultComponents` (htmlOverrides convention) is re-exported from the
// root entry point so `import { defaultComponents } from "zfb"` is the
// canonical access path. Each named override is also re-exported so
// consumers can tree-shake-import a single one (`import { ContentLink } from "zfb"`)
// without dragging in the whole map.
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
