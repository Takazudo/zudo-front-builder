// Public type surface for the zfb island system.
//
// `When` is the string-literal union of allowed `data-when` values. The
// hydration runtime (Sub 3) reads the same attribute and dispatches on the
// same set of strings. Keep these in sync; the unit tests pin the spelling.

/**
 * Hydration scheduling strategy.
 *
 * - `"visible"` — hydrate when the island first intersects the viewport
 *   (IntersectionObserver, threshold 0). Cheapest deferral for content the
 *   user has not scrolled to yet.
 * - `"idle"` — hydrate during the next idle callback. Falls back to a
 *   `setTimeout(0)` on platforms without `requestIdleCallback`.
 * - `"load"` — hydrate immediately and synchronously. Default when
 *   `when` is omitted.
 */
export type When = "visible" | "idle" | "load";

/** Public set of allowed `When` values, useful for runtime validation. */
export const WHEN_VALUES: readonly When[] = ["visible", "idle", "load"];

/** Default `when` strategy applied when `when` is omitted. */
export const DEFAULT_WHEN: When = "load";

/** Type guard for `When`. */
export function isWhen(value: unknown): value is When {
  return typeof value === "string" && (WHEN_VALUES as readonly string[]).includes(value);
}
