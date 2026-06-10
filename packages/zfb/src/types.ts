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
 * - `"media"` — hydrate when a CSS media query first matches (matchMedia).
 *   Requires the companion `media` prop on `<Island>`. Degrades to immediate
 *   hydration when `window.matchMedia` is unavailable.
 */
export type When = "visible" | "idle" | "load" | "media";

/** Public set of allowed `When` values, useful for runtime validation. */
export const WHEN_VALUES: readonly When[] = ["visible", "idle", "load", "media"];

/** Default `when` strategy applied when `when` is omitted. */
export const DEFAULT_WHEN: When = "load";

/** Type guard for `When`. */
export function isWhen(value: unknown): value is When {
  return typeof value === "string" && (WHEN_VALUES as readonly string[]).includes(value);
}

/**
 * Resolve the validated `when` value, warning in development for unknown
 * inputs. Returns the input unchanged when valid, or [`DEFAULT_WHEN`]
 * otherwise. Lives here (rather than in `island.ts`) so the runtime
 * scheduler can import it without transitively pulling in the JSX
 * wrapper.
 */
export function resolveWhen(when: unknown): When {
  if (when === undefined) return DEFAULT_WHEN;
  if (isWhen(when)) return when;
  if (typeof process !== "undefined" && process.env && process.env["NODE_ENV"] !== "production") {
    // eslint-disable-next-line no-console
    console.warn(
      `[zfb] <Island when="${String(when)}"> is not a valid value. ` +
        `Expected "visible" | "idle" | "load" | "media". Falling back to "${DEFAULT_WHEN}".`,
    );
  }
  return DEFAULT_WHEN;
}
