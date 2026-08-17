/// <reference lib="dom" />
// zfb-only addition (no Astro upstream — #2424).
//
// Inside an `about:srcdoc` document (e.g. an SPA-preview iframe shell),
// Chromium refuses `history.replaceState`/`pushState` and throws. The router
// calls these unconditionally — at module init and from several
// navigation-adjacent paths — so a zfb page loaded via `srcdoc` would break
// on the very first call. These wrappers swallow that failure so the router
// degrades silently instead of propagating the throw.
//
// In a normal document the History API never throws here, so the try/catch
// is a no-op passthrough: same call, same arguments, same behavior.
//
// `...args` (rather than naming `data`/`unused`/`url`) preserves the exact
// call arity — several call sites omit the trailing `url` argument, and
// spreading a shorter tuple calls the native method with that same shorter
// arity rather than passing an explicit `undefined`.

export function safeReplaceState(...args: Parameters<History["replaceState"]>): void {
  try {
    history.replaceState(...args);
  } catch {
    // e.g. about:srcdoc — degrade silently, see file header.
  }
}

export function safePushState(...args: Parameters<History["pushState"]>): void {
  try {
    history.pushState(...args);
  } catch {
    // e.g. about:srcdoc — degrade silently, see file header.
  }
}
