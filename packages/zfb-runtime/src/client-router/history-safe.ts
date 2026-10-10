/// <reference lib="dom" />
// zfb-only addition (no Astro upstream — #2424, #4127).
//
// The browser can refuse a History write for two very different reasons:
//
//   - Inside an `about:srcdoc` document (e.g. an SPA-preview iframe shell),
//     Chromium refuses `history.replaceState`/`pushState` with a SecurityError.
//     The router must keep working there, so that restriction is TOLERATED.
//   - In an ordinary document the write can still be REJECTED — WebKit, for
//     example, throws once a page exceeds its History-write rate limit (#4125).
//     The URL did not change, so a caller that goes on as if it had would leave
//     the page content, the address bar and the router bookkeeping disagreeing.
//
// `tryPushState`/`tryReplaceState` report which of the three outcomes happened
// so navigation code can advance its bookkeeping only on a committed write.
// `safeReplaceState` is for metadata-only writes (the init seed, scroll-position
// saves) whose existing contract is best-effort: it swallows every failure.
//
// `...args` (rather than naming `data`/`unused`/`url`) preserves the exact
// call arity — several call sites omit the trailing `url` argument, and
// spreading a shorter tuple calls the native method with that same shorter
// arity rather than passing an explicit `undefined`.

export type HistoryWriteResult =
  | { readonly outcome: "committed" }
  | { readonly outcome: "tolerated"; readonly error: unknown }
  | { readonly outcome: "rejected"; readonly error: unknown };

const COMMITTED: HistoryWriteResult = { outcome: "committed" };

// Identify srcdoc by the document's own address. `document.baseURI` is
// deliberately NOT consulted: a srcdoc document inherits its parent's base URL,
// so an HTTP baseURI says nothing about which kind of document this is.
const isSrcdocDocument = (): boolean => document.URL === "about:srcdoc";

const isSecurityError = (error: unknown): boolean =>
  typeof error === "object" &&
  error !== null &&
  (error as { name?: unknown }).name === "SecurityError";

function classify(error: unknown): HistoryWriteResult {
  // A SecurityError alone is not proof of srcdoc — an ordinary document can
  // throw one too — so both the document identity and the error must match.
  return isSrcdocDocument() && isSecurityError(error)
    ? { outcome: "tolerated", error }
    : { outcome: "rejected", error };
}

export function tryReplaceState(...args: Parameters<History["replaceState"]>): HistoryWriteResult {
  try {
    history.replaceState(...args);
    return COMMITTED;
  } catch (error) {
    return classify(error);
  }
}

export function tryPushState(...args: Parameters<History["pushState"]>): HistoryWriteResult {
  try {
    history.pushState(...args);
    return COMMITTED;
  } catch (error) {
    return classify(error);
  }
}

export function safeReplaceState(...args: Parameters<History["replaceState"]>): void {
  tryReplaceState(...args);
}
