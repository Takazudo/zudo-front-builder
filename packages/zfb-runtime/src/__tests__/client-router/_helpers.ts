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

export interface DocumentLoadCapture {
  assigned: string[];
  replaced: string[];
  /** Put every overridden `location` property back exactly as it was. Safe to call twice. */
  restore: () => void;
}

const CAPTURED_LOCATION_PROPS = ["href", "replace"] as const;
type CapturedLocationProperty = (typeof CAPTURED_LOCATION_PROPS)[number];

function restoreLocationProperty(
  property: CapturedLocationProperty,
  descriptor: PropertyDescriptor | undefined,
): void {
  if (descriptor) Object.defineProperty(location, property, descriptor);
  else delete (location as unknown as Record<string, unknown>)[property];
}

function findPrototypeHrefGetter(): () => string {
  let proto: object | null = Object.getPrototypeOf(location);
  while (proto) {
    const getter = Object.getOwnPropertyDescriptor(proto, "href")?.get;
    if (getter) return getter;
    proto = Object.getPrototypeOf(proto);
  }
  throw new Error("captureDocumentLoads: no `href` getter found on location's prototype chain");
}

/**
 * Capture the router's full-document loads instead of letting happy-dom
 * perform them: `location.href = …` assignments and `location.replace(…)`
 * calls are recorded, while reading `location.href` keeps reporting the live
 * URL (pushState/replaceState still move it). Call the returned `restore` from
 * afterEach: it reinstates each property's original OWN descriptor, or its
 * original absence, so a later test sees happy-dom's prototype getter again
 * (#4130).
 */
export function captureDocumentLoads(): DocumentLoadCapture {
  const originalDescriptors = new Map<string, PropertyDescriptor | undefined>(
    CAPTURED_LOCATION_PROPS.map((prop) => [prop, Object.getOwnPropertyDescriptor(location, prop)]),
  );
  const liveHref = findPrototypeHrefGetter();
  const assigned: string[] = [];
  const replaced: string[] = [];
  let hrefInstalled = false;
  try {
    Object.defineProperty(location, "href", {
      configurable: true,
      get: () => liveHref.call(location),
      set: (value: string) => {
        assigned.push(String(value));
      },
    });
    hrefInstalled = true;
    Object.defineProperty(location, "replace", {
      configurable: true,
      writable: true,
      value: (url: string | URL) => {
        replaced.push(String(url));
      },
    });
  } catch (error) {
    // Installing `replace` can fail independently (for example, if it already
    // has a non-configurable own descriptor). Do not leave the `href` shim
    // behind when that second installation fails.
    if (hrefInstalled) restoreLocationProperty("href", originalDescriptors.get("href"));
    throw error;
  }
  let restored = false;
  return {
    assigned,
    replaced,
    restore: () => {
      if (restored) return;
      for (const [prop, descriptor] of originalDescriptors) {
        restoreLocationProperty(prop as CapturedLocationProperty, descriptor);
      }
      restored = true;
    },
  };
}
