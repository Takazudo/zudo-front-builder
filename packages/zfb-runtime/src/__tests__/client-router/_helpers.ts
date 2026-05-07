// Shared test helpers for client-router test files.
//
// Each test file using a happy-dom environment imports `installHappyDomShim()`
// at module top to ensure `<link rel=stylesheet>` and `<script src>` insertions
// don't trigger real fetches, and to give the suite a consistent way to drain
// async tasks in afterEach.

type HappyDOMWindow = Window & {
  happyDOM: {
    settings: Record<string, boolean>;
    waitUntilComplete: () => Promise<void>;
  };
};

let cachedHappyWindow: HappyDOMWindow | undefined;

function getHappyWindow(): HappyDOMWindow {
  if (cachedHappyWindow) return cachedHappyWindow;
  const w = window as unknown as HappyDOMWindow;
  cachedHappyWindow = w;
  return w;
}

/**
 * Apply happy-dom runtime settings that disable network loads and treat
 * disabled loads as silent successes. Must be called at module top so the
 * settings are in place before any DOM mutation happens in describe/beforeEach.
 */
export function installHappyDomShim(): void {
  const w = getHappyWindow();
  w.happyDOM.settings["disableJavaScriptFileLoading"] = true;
  w.happyDOM.settings["disableCSSFileLoading"] = true;
  w.happyDOM.settings["disableIframePageLoading"] = true;
  w.happyDOM.settings["handleDisabledFileLoadingAsSuccess"] = true;
}

/**
 * Drain happy-dom's pending async tasks (e.g. silent stylesheet load events).
 * Call from afterEach so async work doesn't race vitest's environment teardown.
 */
export async function drainHappyDom(): Promise<void> {
  await getHappyWindow().happyDOM.waitUntilComplete();
}

/**
 * Reinstall a fresh <body> on the live document. Use in beforeEach when a
 * previous test may have called swapBodyElement() — that path replaces the
 * original body via `replaceWith()`, after which `document.body.innerHTML = ""`
 * is a not-a-fresh-element shortcut whose getElementById internals can be
 * stale. Recreating the element through the standard DOM API resets cleanly.
 */
export function resetDocument(): void {
  document.head.innerHTML = "";
  while (document.documentElement.attributes.length > 0) {
    document.documentElement.removeAttribute(document.documentElement.attributes[0]!.name);
  }
  if (document.body) document.body.remove();
  document.documentElement.appendChild(document.createElement("body"));
}

/**
 * Build a Document via DOMParser. Used by tests that need to feed
 * swap/loader functions a fresh-from-the-network "incoming" doc.
 */
export function htmlDoc(html: string): Document {
  return new DOMParser().parseFromString(html, "text/html");
}
