// Hydration scheduling helper consumed by the hydration runtime (Sub 3).
//
// Sub 3 owns the hydration runtime that walks the DOM, finds elements
// marked with `data-zfb-island`, and dispatches each one through this
// helper to decide *when* to fire the actual hydrate() call. The helper
// itself does not know how to hydrate — it only schedules the supplied
// `fire` callback.
//
// The branching matches the `When` union exactly:
//   "visible" → IntersectionObserver, threshold 0.0, hydrate on first
//                intersection, then disconnect.
//   "idle"    → requestIdleCallback if available, otherwise setTimeout(0).
//   "load"    → immediate, synchronous fire.
//
// Anything else is treated as "load" (with a console.warn in development).
// The helper is environment-tolerant: callers can run it in jsdom /
// happy-dom or bare Node, and the absence of `IntersectionObserver` /
// `requestIdleCallback` is handled gracefully.

import { resolveWhen } from "./island.js";
import type { When } from "./types.js";

/** Window-shaped subset that the helper actually uses. */
type IdleWindow = typeof globalThis & {
  requestIdleCallback?: (
    cb: (deadline: { didTimeout: boolean; timeRemaining: () => number }) => void,
    options?: { timeout?: number },
  ) => number;
};

/**
 * Schedule a hydration `fire` callback for `target` according to `when`.
 *
 * Returns a `cancel` function that aborts the scheduling if it has not
 * fired yet. After firing, calling `cancel` is a no-op. If the helper
 * cannot find the relevant browser API (e.g. running in pure Node with no
 * polyfill), it falls back to firing synchronously so server-side smoke
 * tests still observe the call.
 */
export function scheduleHydrate(
  target: Element,
  when: When | string | undefined,
  fire: () => void,
): () => void {
  const resolved = resolveWhen(when);

  if (resolved === "load") {
    fire();
    return noop;
  }

  if (resolved === "idle") {
    return scheduleIdle(fire);
  }

  // "visible"
  return scheduleVisible(target, fire);
}

function noop(): void {
  // intentionally empty
}

function scheduleIdle(fire: () => void): () => void {
  const w = globalThis as IdleWindow;
  let cancelled = false;
  let fired = false;
  const wrapped = (): void => {
    if (cancelled || fired) return;
    fired = true;
    fire();
  };

  if (typeof w.requestIdleCallback === "function") {
    const handle = w.requestIdleCallback(wrapped);
    return () => {
      if (fired) return;
      cancelled = true;
      const cancelIdle = (
        globalThis as typeof globalThis & {
          cancelIdleCallback?: (handle: number) => void;
        }
      ).cancelIdleCallback;
      if (typeof cancelIdle === "function") cancelIdle(handle);
    };
  }

  const handle = setTimeout(wrapped, 0);
  return () => {
    if (fired) return;
    cancelled = true;
    clearTimeout(handle);
  };
}

function scheduleVisible(target: Element, fire: () => void): () => void {
  const Observer = (
    globalThis as typeof globalThis & {
      IntersectionObserver?: typeof IntersectionObserver;
    }
  ).IntersectionObserver;

  // No IntersectionObserver (e.g. very old browsers, bare Node) — fail
  // open and hydrate immediately so the island is at least functional.
  if (typeof Observer !== "function") {
    fire();
    return noop;
  }

  let fired = false;
  let cancelled = false;
  const observer = new Observer(
    (entries, obs) => {
      if (cancelled || fired) return;
      for (const entry of entries) {
        if (entry.isIntersecting) {
          fired = true;
          obs.disconnect();
          fire();
          return;
        }
      }
    },
    { threshold: 0 },
  );

  observer.observe(target);

  return () => {
    if (fired) return;
    cancelled = true;
    observer.disconnect();
  };
}
