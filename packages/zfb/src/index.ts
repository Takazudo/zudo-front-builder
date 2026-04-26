// Public entry point for the "zfb" package.
//
// User TSX pages reach this module via `import { Island } from "zfb"`.
// The hydration runtime (Sub 3) reaches the helper via
// `import { scheduleHydrate } from "zfb/runtime"` (or by inlining the
// same logic; coordinated separately).

export { Island, resolveWhen, type IslandProps } from "./island.js";
export { scheduleHydrate } from "./runtime.js";
export { DEFAULT_WHEN, isWhen, WHEN_VALUES, type When } from "./types.js";

// Sub 6 will introduce `defaultComponents` (the htmlOverrides convention)
// and re-export it from this module so `import { defaultComponents } from "zfb"`
// is the canonical entry point. Sub 5 only adds the `Content` bridge field
// on `CollectionEntry` (consumed via the `zfb/content` subpath); this
// placeholder marks where the re-export will land without forcing Sub 6
// to invent the location.
//
// TODO(sub-6): export { defaultComponents } from "./default-components.js";
