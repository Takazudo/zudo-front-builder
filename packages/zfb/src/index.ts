// Public entry point for the "zfb" package.
//
// User TSX pages reach this module via `import { Island } from "zfb"`.
// The hydration runtime (Sub 3) reaches the helper via
// `import { scheduleHydrate } from "zfb/runtime"` (or by inlining the
// same logic; coordinated separately).

export { Island, resolveWhen, type IslandProps } from "./island.js";
export { scheduleHydrate } from "./runtime.js";
export { DEFAULT_WHEN, isWhen, WHEN_VALUES, type When } from "./types.js";
