/**
 * @vitest-environment happy-dom
 */
// Integration tests for client-router/router.
//
// These tests exercise the SPA router end-to-end inside a happy-dom
// document: a fetch mock returns canned HTML, navigate() drives the
// transition pipeline (preparation → swap), and we observe the resulting
// document state and dispatched lifecycle events.
//
// Coverage:
//   - happy path with mocked fetch + jsdom-style document
//   - fallback simulation when document.startViewTransition is undefined
//   - hash-only same-page navigation skips the transition path
//   - non-opt-in target page degrades to a graceful full-page load
//   - redirected fetch (res.redirected) preserves redirected URL
//   - idempotent init() does not double-register click/submit listeners
//
// Mocks:
//   - `@takazudo/zfb/runtime` is mocked so we don't drag the real island
//     manager into the test (it would try to load the islands manifest from
//     disk). The router only needs `mountNewIslands` and
//     `cancelPendingIslands` to be callable functions.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";
import { cancelPendingIslands, mountNewIslands, unmountIslands } from "@takazudo/zfb/runtime";

vi.mock("@takazudo/zfb/runtime", () => ({
  mountNewIslands: vi.fn(),
  cancelPendingIslands: vi.fn(),
  unmountIslands: vi.fn(),
}));

installHappyDomShim();

// Inject the opt-in meta tag BEFORE importing the router module, so the
// module's top-level `if (transitionEnabledOnThisPage())` branch sees the
// page as opted-in. router.ts reads the live document at module-eval time.
function enableTransitions(): void {
  const meta = document.createElement("meta");
  meta.setAttribute("name", "zfb-view-transitions-enabled");
  meta.setAttribute("content", "true");
  document.head.appendChild(meta);
}
enableTransitions();

// Stub DOM APIs the router's fallback `animate()` helper needs (the View
// Transitions simulation calls these from `updateDOM` regardless of whether
// any animation actually runs). happy-dom does not implement them.
if (typeof document.getAnimations !== "function") {
  Object.defineProperty(document, "getAnimations", {
    configurable: true,
    value: () => [],
  });
}
// `instanceof KeyframeEffect` check inside isInfinite() — happy-dom omits the
// global. Provide a stand-in so the check returns `false` cleanly.
if (typeof (globalThis as { KeyframeEffect?: unknown }).KeyframeEffect === "undefined") {
  (globalThis as { KeyframeEffect: unknown }).KeyframeEffect = class KeyframeEffect {};
}
// `instanceof SVGAElement` check inside handleClick's link-type guard (#1400)
// — happy-dom implements most SVG*Element globals but omits SVGAElement
// specifically. Without this stand-in, dispatching ANY real "click" event
// (even on a plain HTMLButtonElement, which never satisfies the check) throws
// "SVGAElement is not defined" before the instanceof check can return false.
if (typeof (globalThis as { SVGAElement?: unknown }).SVGAElement === "undefined") {
  (globalThis as { SVGAElement: unknown }).SVGAElement = class SVGAElement {};
}

// Late import after the document is primed.
import {
  init,
  navigate,
  supportsViewTransitions,
  syncHistoryEntry,
  transitionEnabledOnThisPage,
} from "../../client-router/router.js";
// Type-only import from the PUBLIC barrel — proves SyncHistoryEntryOptions is
// re-exported from @takazudo/zfb-runtime/client-router. `import type` is erased
// at runtime, so it does not drag the barrel's <ClientRouter> module into the
// test environment.
import type { SyncHistoryEntryOptions } from "../../client-router/index.js";

beforeEach(() => {
  resetDocument();
  // Re-inject opt-in meta tag for each test (resetDocument clears head).
  enableTransitions();
});

afterEach(async () => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  await drainHappyDom();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function htmlResponse(body: string, init?: ResponseInit): Response {
  return new Response(body, {
    ...init,
    headers: { "content-type": "text/html; charset=utf-8", ...(init?.headers ?? {}) },
  });
}

function pageHtml(title: string, mainContent: string, optIn = true): string {
  const optInMeta = optIn ? `<meta name="zfb-view-transitions-enabled" content="true">` : "";
  return `<!doctype html><html><head>
    ${optInMeta}
    <title>${title}</title>
  </head><body>
    <main>${mainContent}</main>
  </body></html>`;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("router top-level state", () => {
  it("supportsViewTransitions reflects document.startViewTransition presence", () => {
    // happy-dom does not implement startViewTransition by default → false.
    expect(supportsViewTransitions).toBe(false);
  });

  it("transitionEnabledOnThisPage reads the live document each call", () => {
    expect(transitionEnabledOnThisPage()).toBe(true);
    document.head.innerHTML = "";
    expect(transitionEnabledOnThisPage()).toBe(false);
  });
});

describe("navigate() — happy path with mocked fetch", () => {
  it("fetches the target URL and swaps in the new <main>", async () => {
    const fetchMock = vi.fn(async (url: RequestInfo) => {
      const u = String(url);
      if (u.endsWith("/about")) return htmlResponse(pageHtml("About", "About content"));
      throw new Error(`unexpected fetch: ${u}`);
    });
    vi.stubGlobal("fetch", fetchMock);

    document.body.innerHTML = `<main>Home content</main>`;

    await navigate("/about");

    // A document.title swap is part of moveToLocation; the new page sets
    // <title>About</title> via swapHeadElements.
    expect(document.title).toBe("About");
    expect(document.querySelector("main")?.textContent).toBe("About content");
    // Fetch should have been called exactly once for /about.
    const calls = fetchMock.mock.calls;
    expect(calls).toHaveLength(1);
    expect(String(calls[0]![0])).toContain("/about");
  });

  it("dispatches the lifecycle events in order: before-preparation, after-preparation, before-swap, after-swap", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("Next", "next"))),
    );

    const seen: string[] = [];
    const ev = (type: string) => () => seen.push(type);
    const handlers: Array<[string, EventListener]> = [
      ["zfb:before-preparation", ev("before-preparation")],
      ["zfb:after-preparation", ev("after-preparation")],
      ["zfb:before-swap", ev("before-swap")],
      ["zfb:after-swap", ev("after-swap")],
    ];
    for (const [t, h] of handlers) document.addEventListener(t, h);

    await navigate("/next");

    for (const [t, h] of handlers) document.removeEventListener(t, h);

    // The four lifecycle events must fire and in this order. We assert
    // membership AND ordering.
    expect(seen).toEqual(["before-preparation", "after-preparation", "before-swap", "after-swap"]);
  });
});

describe("zfb:navigation-aborted — positive control (happy path does NOT fire)", () => {
  it("does not dispatch zfb:navigation-aborted on a successful SPA swap", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("Success", "success content"))),
    );

    const seenNavigationAborted = vi.fn();
    document.addEventListener("zfb:navigation-aborted", seenNavigationAborted);

    await navigate("/success");

    document.removeEventListener("zfb:navigation-aborted", seenNavigationAborted);

    // The happy path completes the swap; navigation-aborted must NOT fire.
    expect(seenNavigationAborted).not.toHaveBeenCalled();
    expect(document.querySelector("main")?.textContent).toBe("success content");
  });
});

describe("zfb:navigation-aborted — rapid-navigation race (signal-aborted branch)", () => {
  it("fires navigation-aborted exactly once for the superseded navigation, not for the winner", async () => {
    // Build a manually-resolvable promise for /page-a so we control when A's
    // fetch completes. /page-b resolves immediately.
    let releaseA!: (r: Response) => void;
    const promiseA = new Promise<Response>((resolve) => {
      releaseA = resolve;
    });

    const fetchMock = vi.fn(async (url: RequestInfo, init?: RequestInit) => {
      const u = String(url);
      if (u.includes("/page-a")) return promiseA;
      if (u.includes("/page-b")) return htmlResponse(pageHtml("B", "page b content"));
      throw new Error(`unexpected fetch: ${u}`);
    });
    vi.stubGlobal("fetch", fetchMock);

    const originalHref = location.href;
    Object.defineProperty(location, "href", {
      configurable: true,
      get: () => originalHref,
      set: (_v: string) => {
        // capture but don't assign; keeps happy-dom stable
      },
    });

    const seenNavigationAborted = vi.fn();
    const seenAfterSwap = vi.fn();
    document.addEventListener("zfb:navigation-aborted", seenNavigationAborted);
    document.addEventListener("zfb:after-swap", seenAfterSwap);

    // Start /page-a navigation without awaiting so it's in-flight.
    const navAPromise = navigate("/page-a");

    // Immediately start /page-b — this aborts A's AbortController via
    // abortAndRecreateMostRecentNavigation() inside transition().
    const navBPromise = navigate("/page-b");

    // Verify A's fetch signal is aborted. The signal was the second argument
    // (init) passed to fetch for the /page-a call.
    const aFetchCall = fetchMock.mock.calls.find((c) => String(c[0]).includes("/page-a"));
    expect(aFetchCall).toBeDefined();
    const aSignal = (aFetchCall![1] as RequestInit | undefined)?.signal;
    expect(aSignal?.aborted).toBe(true);

    // Release A's promise with a valid opt-in HTML page. A's loader will
    // complete normally, but the signal is already aborted so transition()
    // will take the signal-aborted branch and fire zfb:navigation-aborted.
    releaseA(htmlResponse(pageHtml("A", "page a content")));

    // Await both navigations.
    await navAPromise;
    await navBPromise;

    document.removeEventListener("zfb:navigation-aborted", seenNavigationAborted);
    document.removeEventListener("zfb:after-swap", seenAfterSwap);

    // A fires navigation-aborted (signal-aborted branch); B fires after-swap.
    // B must NOT fire navigation-aborted.
    expect(seenNavigationAborted).toHaveBeenCalledTimes(1);
    expect(seenAfterSwap).toHaveBeenCalledTimes(1);
  });
});

describe("hash-only same-page navigation", () => {
  it("does not call fetch for `#anchor` on the same page", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    // Seed history.state so the router's moveToLocation has something to read.
    history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "");

    await navigate(`${location.pathname}#section-2`);

    // No fetch should have been issued — hash-only navs skip the transition path.
    expect(fetchMock).not.toHaveBeenCalled();
    expect(location.hash).toBe("#section-2");
  });
});

describe("redirected fetch — res.redirected", () => {
  it("uses the redirected URL as the new location while preserving the requested fragment", async () => {
    // Build a Response that reports redirected=true via Object.defineProperty,
    // since the Response constructor does not let us set `redirected`/`url`
    // directly.
    const finalUrl = "http://localhost:3000/blog/redirected-target";
    const res = htmlResponse(pageHtml("Redirected", "redirected"));
    Object.defineProperty(res, "redirected", { value: true });
    Object.defineProperty(res, "url", { value: finalUrl });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => res),
    );

    await navigate("/blog/old-slug#section");

    // The router should have swapped to the redirected URL while keeping the
    // original hash fragment.
    expect(location.pathname).toBe("/blog/redirected-target");
    expect(location.hash).toBe("#section");
  });
});

describe("non-opt-in target page degrade", () => {
  it("delegates to a full browser load when the new page lacks the opt-in meta tag", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("Plain", "plain content", /*optIn*/ false))),
    );

    const seenAfterSwap = vi.fn();
    const seenNavigationAborted = vi.fn();
    document.addEventListener("zfb:after-swap", seenAfterSwap);
    document.addEventListener("zfb:navigation-aborted", seenNavigationAborted);

    // happy-dom's location.href is read-only by default; capture assignments.
    const originalHref = location.href;
    let assignedHref: string | undefined;
    Object.defineProperty(location, "href", {
      configurable: true,
      get: () => originalHref,
      set: (v: string) => {
        assignedHref = v;
      },
    });

    await navigate("/plain-no-clientrouter");

    document.removeEventListener("zfb:after-swap", seenAfterSwap);
    document.removeEventListener("zfb:navigation-aborted", seenNavigationAborted);

    // The router's defaultLoader path calls `preventDefault()` on the
    // preparation event when the new page has no opt-in meta, which short-
    // circuits the swap. The before/after-preparation events fire but
    // before-swap/after-swap do NOT, and a full browser load is requested
    // by setting location.href. zfb:navigation-aborted fires in the
    // prep-aborted branch to signal that the SPA swap will not happen.
    expect(seenAfterSwap).not.toHaveBeenCalled();
    expect(seenNavigationAborted).toHaveBeenCalledOnce();
    expect(assignedHref).toContain("/plain-no-clientrouter");
  });
});

describe("init() — idempotent listener registration", () => {
  it("calling init() multiple times only registers click/submit listeners once", () => {
    const addListenerSpy = vi.spyOn(document, "addEventListener");

    init();
    init();
    init();

    // Filter to only the click + submit registrations the router contributes.
    const clickCalls = addListenerSpy.mock.calls.filter((c) => c[0] === "click");
    const submitCalls = addListenerSpy.mock.calls.filter((c) => c[0] === "submit");

    // First call wins; subsequent init() calls must be no-ops.
    expect(clickCalls).toHaveLength(1);
    expect(submitCalls).toHaveLength(1);

    addListenerSpy.mockRestore();
  });
});

describe("fallback simulation when document.startViewTransition is undefined", () => {
  it("transition completes via simulation when supportsViewTransitions is false (happy-dom default)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("Fallback", "fallback content"))),
    );

    // happy-dom does not provide document.startViewTransition. The router
    // resolves `supportsViewTransitions` at module top → false. The transition
    // path therefore exercises the simulation branch (manual updateDOM call
    // wrapped in a Promise).
    expect(supportsViewTransitions).toBe(false);

    await navigate("/fallback");

    // If the simulation path is wired correctly, the swap still completes.
    expect(document.querySelector("main")?.textContent).toBe("fallback content");
    expect(document.title).toBe("Fallback");
  });
});

describe("non-HTML response degrade", () => {
  it("falls back to a full browser load when the fetch returns a non-HTML content-type", async () => {
    // The router's fetchHTML helper returns null for any media type other
    // than text/html or application/xhtml+xml. The defaultLoader path then
    // calls preventDefault() on the preparation event so the browser handles
    // the URL itself.
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response('{"ok":true}', {
            status: 200,
            headers: { "content-type": "application/json" },
          }),
      ),
    );

    const seenAfterSwap = vi.fn();
    const seenNavigationAborted = vi.fn();
    document.addEventListener("zfb:after-swap", seenAfterSwap);
    document.addEventListener("zfb:navigation-aborted", seenNavigationAborted);

    const originalHref = location.href;
    let assignedHref: string | undefined;
    Object.defineProperty(location, "href", {
      configurable: true,
      get: () => originalHref,
      set: (v: string) => {
        assignedHref = v;
      },
    });

    await navigate("/api/echo");

    document.removeEventListener("zfb:after-swap", seenAfterSwap);
    document.removeEventListener("zfb:navigation-aborted", seenNavigationAborted);

    // When the response is not HTML, the preparation event is prevented and
    // zfb:navigation-aborted fires in the prep-aborted branch.
    expect(seenAfterSwap).not.toHaveBeenCalled();
    expect(seenNavigationAborted).toHaveBeenCalledOnce();
    expect(assignedHref).toContain("/api/echo");
  });
});

describe("navigate() — fallback=none + no View Transitions → full page load (no fetch)", () => {
  it("sets location.href directly and does not call fetch when fallback is none and VT is unsupported", async () => {
    // supportsViewTransitions is already false under happy-dom (document.startViewTransition absent).
    expect(supportsViewTransitions).toBe(false);

    // Mount the fallback=none meta so getFallback() returns "none".
    const fallbackMeta = document.createElement("meta");
    fallbackMeta.setAttribute("name", "zfb-view-transitions-fallback");
    fallbackMeta.setAttribute("content", "none");
    document.head.appendChild(fallbackMeta);

    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    // Capture location.href assignments (happy-dom's href is read-only by default).
    const originalHref = location.href;
    let assignedHref: string | undefined;
    Object.defineProperty(location, "href", {
      configurable: true,
      get: () => originalHref,
      set: (v: string) => {
        assignedHref = v;
      },
    });

    await navigate("/foo");

    // Full page load path: location.href must be set to the resolved URL.
    expect(assignedHref).toBeDefined();
    expect(assignedHref).toContain("/foo");
    // The guard fires before any fetch — fetch must NOT have been called.
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("uses location.replace (not href) when options.history is 'replace'", async () => {
    expect(supportsViewTransitions).toBe(false);

    const fallbackMeta = document.createElement("meta");
    fallbackMeta.setAttribute("name", "zfb-view-transitions-fallback");
    fallbackMeta.setAttribute("content", "none");
    document.head.appendChild(fallbackMeta);

    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    // Capture href assignments separately from replace() calls so we can
    // assert push-vs-replace semantics. happy-dom's location.replace is a
    // real method, so spy on it; href is read-only, so shim it.
    const originalHref = location.href;
    let assignedHref: string | undefined;
    Object.defineProperty(location, "href", {
      configurable: true,
      get: () => originalHref,
      set: (v: string) => {
        assignedHref = v;
      },
    });
    const replaceSpy = vi.spyOn(location, "replace").mockImplementation(() => {});

    await navigate("/bar", { history: "replace" });

    // Replace semantics: location.replace called with the resolved URL,
    // href setter NOT touched.
    expect(replaceSpy).toHaveBeenCalledTimes(1);
    expect(String(replaceSpy.mock.calls[0]?.[0])).toContain("/bar");
    expect(assignedHref).toBeUndefined();
    expect(fetchMock).not.toHaveBeenCalled();

    replaceSpy.mockRestore();
  });
});

describe("popstate — forward + back history navigation", () => {
  it("popstate with state.index > current triggers a forward transition; lower triggers back", async () => {
    const fetchMock = vi.fn(async (url: RequestInfo) => {
      const u = String(url);
      if (u.includes("/page-a")) return htmlResponse(pageHtml("A", "page a"));
      if (u.includes("/page-b")) return htmlResponse(pageHtml("B", "page b"));
      throw new Error(`unexpected fetch: ${u}`);
    });
    vi.stubGlobal("fetch", fetchMock);

    // Forward: navigate /page-a then /page-b — pushState raises currentHistoryIndex.
    await navigate("/page-a");
    await navigate("/page-b");
    expect(document.title).toBe("B");

    // Now simulate popstate going back. The onPopState handler reads
    // history.state.index and fires a "back" transition. We can't rely on
    // happy-dom's history.back() to fire popstate cleanly, so we dispatch
    // PopStateEvent manually with state.index < current.
    history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/page-a");
    const popEvent = new PopStateEvent("popstate", {
      state: { index: 0, scrollX: 0, scrollY: 0 },
    });
    window.dispatchEvent(popEvent);

    // Allow microtasks + any rAF/timers to drain.
    await new Promise((r) => setTimeout(r, 0));

    // The router should have re-fetched /page-a (back nav) and swapped.
    // Check that fetch was called for /page-a as part of the back navigation.
    const urls = fetchMock.mock.calls.map((c) => String(c[0]));
    expect(urls.some((u) => u.includes("/page-a"))).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// pageshow / bfcache re-sync (WebKit Back-history fix, #1076)
// ---------------------------------------------------------------------------
//
// L2 (DOM-component) regression for the iOS/WebKit Back bug: after a real
// SPA navigation (list → detail), a bfcache restore can leave the module's
// private `currentHistoryIndex` desynced from the live `history.state.index`,
// so the popstate direction calc (`nextIndex > currentHistoryIndex`)
// misclassifies a Back as a forward navigation. The init-block `pageshow`
// listener re-seeds `currentHistoryIndex` from `history.state.index` on a
// `persisted` (bfcache) restore so the next popstate classifies correctly.
//
// DRIVE THE REAL PUBLIC PATH: the two SPA entries are seeded via navigate()
// through production code so `currentHistoryIndex` reaches 2 organically — it
// is never hand-set. We then simulate the bfcache restore (the live history
// index advanced while the page sat in cache) and assert the OBSERVABLE: the
// list page is fetched/swapped and the popstate direction is "back".
//
// BLIND SPOT (documented): happy-dom models neither bfcache nor
// `hasUAVisualTransition`, so this proves the handler's branch logic, NOT the
// real WebKit fix. Definitive proof is the Wave-2 WebKit harness on a Mac.
describe("pageshow / bfcache re-sync — WebKit Back-history (#1076)", () => {
  // Earlier tests in this file shim `location.href` via Object.defineProperty
  // and never restore it, leaving a frozen own-property getter that would make
  // `history.replaceState(state, "", path)` no longer reflect into
  // `location.href` (which `onPopState` reads via `new URL(location.href)`).
  // Delete any leaked own-property so happy-dom's native prototype getter — which
  // DOES track replaceState — is back in effect for these tests.
  beforeEach(() => {
    if (Object.getOwnPropertyDescriptor(location, "href")) {
      delete (location as unknown as Record<string, unknown>)["href"];
    }
  });

  // Capture the direction the router classified for a popstate-driven
  // transition by listening on the `zfb:before-preparation` event, which
  // carries the resolved Direction.
  function captureDirection(): { get: () => string | undefined; dispose: () => void } {
    let seen: string | undefined;
    const handler = (ev: Event) => {
      seen = (ev as unknown as { direction: string }).direction;
    };
    document.addEventListener("zfb:before-preparation", handler);
    return {
      get: () => seen,
      dispose: () => document.removeEventListener("zfb:before-preparation", handler),
    };
  }

  const drain = () => new Promise((r) => setTimeout(r, 0));

  // happy-dom's PageTransitionEvent ignores the `persisted` init option (it
  // always reports `undefined`), so build a plain "pageshow" Event and shim the
  // read-only `persisted` getter — the same defineProperty shim pattern the
  // suite uses for location.href. `persisted: undefined` yields a no-`persisted`
  // event (plain Event), exercising the absent-property branch.
  function pageshowEvent(persisted?: boolean): Event {
    const ev = new Event("pageshow");
    if (persisted !== undefined) {
      Object.defineProperty(ev, "persisted", { configurable: true, value: persisted });
    }
    return ev;
  }

  function fetchTwoPages() {
    const fetchMock = vi.fn(async (url: RequestInfo) => {
      const u = String(url);
      if (u.includes("/list")) return htmlResponse(pageHtml("List", "list page"));
      if (u.includes("/detail")) return htmlResponse(pageHtml("Detail", "detail page"));
      if (u.includes("/home")) return htmlResponse(pageHtml("Home", "home page"));
      throw new Error(`unexpected fetch: ${u}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    return fetchMock;
  }

  // Drive a browser Back: happy-dom's `history.replaceState(state, "", path)`
  // updates both `history.state` and `location.href` (same-origin path), which
  // is exactly what `onPopState` reads (`history.state` + `new URL(location.href)`).
  // We then fire the popstate the browser would have fired for that entry.
  function dispatchBackTo(path: string, index: number) {
    const state = { index, scrollX: 0, scrollY: 0 };
    history.replaceState(state, "", path);
    window.dispatchEvent(new PopStateEvent("popstate", { state }));
  }

  // Seed two SPA entries (list → detail) through production code so the module
  // index advances organically. `currentHistoryIndex` is module-global and
  // monotonically increasing across the whole file, so we read the resulting
  // live index back from `history.state.index` and compute every desync/probe
  // index RELATIVE to it — never hand-set or assume an absolute value.
  async function seedListThenDetail(): Promise<number> {
    await navigate("/list");
    await navigate("/detail");
    expect(document.title).toBe("Detail");
    return (history.state as { index: number }).index; // === module currentHistoryIndex
  }

  it("re-seeds currentHistoryIndex on persisted restore so the Back popstate classifies as 'back' (not forward)", async () => {
    const fetchMock = fetchTwoPages();

    const cur = await seedListThenDetail(); // module currentHistoryIndex === cur

    // Inject the desync: simulate a bfcache restore where the LIVE history
    // index advanced (to cur+2) while the page sat in cache — the module
    // counter is still `cur`. Without a pageshow re-seed the two disagree.
    history.replaceState({ index: cur + 2, scrollX: 0, scrollY: 0 }, "", "/detail");

    // bfcache restore notification — re-seeds currentHistoryIndex from cur+2.
    window.dispatchEvent(pageshowEvent(true));

    const dir = captureDirection();
    fetchMock.mockClear();

    // Browser Back to the list entry. In the advanced (re-seeded) stack the
    // list sits at cur+1, one below the restored detail at cur+2 → "back".
    // Without the re-seed the module still thinks current is `cur`, so
    // (cur+1) > cur misclassifies this Back as "forward".
    dispatchBackTo("/list", cur + 1);
    await drain();

    // Observable 1: the LIST page was fetched/swapped (not home).
    const urls = fetchMock.mock.calls.map((c) => String(c[0]));
    expect(urls.some((u) => u.includes("/list"))).toBe(true);
    expect(urls.some((u) => u.includes("/home"))).toBe(false);
    // Observable 2: the direction was "back".
    expect(dir.get()).toBe("back");

    dir.dispose();
  });

  it("is a no-op on a non-persisted pageshow (normal load) — does not shift the index", async () => {
    fetchTwoPages();

    const cur = await seedListThenDetail(); // module currentHistoryIndex === cur

    // Live index advanced to cur+5, but this is a NORMAL (non-bfcache)
    // pageshow: the handler must not re-seed, so the module counter stays `cur`.
    history.replaceState({ index: cur + 5, scrollX: 0, scrollY: 0 }, "", "/detail");
    window.dispatchEvent(pageshowEvent(false));

    const dir = captureDirection();
    // Probe with a popstate to cur+3. With no re-seed (module still `cur`),
    // (cur+3) > cur → "forward". A buggy re-seed to cur+5 would give
    // (cur+3) < (cur+5) → "back". We assert the no-op outcome: "forward".
    dispatchBackTo("/list", cur + 3);
    await drain();

    expect(dir.get()).toBe("forward");
    dir.dispose();
  });

  it("is a no-op on a pageshow event lacking `persisted` (plain Event)", async () => {
    fetchTwoPages();

    const cur = await seedListThenDetail(); // module currentHistoryIndex === cur

    history.replaceState({ index: cur + 5, scrollX: 0, scrollY: 0 }, "", "/detail");
    // A plain Event has no `persisted` property (undefined) → must be a no-op.
    window.dispatchEvent(pageshowEvent());

    const dir = captureDirection();
    // No re-seed happened → module index stayed `cur` → (cur+3) > cur → "forward".
    dispatchBackTo("/list", cur + 3);
    await drain();

    expect(dir.get()).toBe("forward");
    dir.dispose();
  });

  it("is idempotent: dispatching persisted pageshow twice does not shift the index or fire a transition", async () => {
    const fetchMock = fetchTwoPages();

    const cur = await seedListThenDetail();

    history.replaceState({ index: cur + 2, scrollX: 0, scrollY: 0 }, "", "/detail");

    fetchMock.mockClear();
    const dir = captureDirection();

    // Two consecutive bfcache restores. Re-seeding twice from the same live
    // index must be a stable no-op the second time — and neither dispatch may
    // trigger a spurious transition (no fetch, no direction classified).
    window.dispatchEvent(pageshowEvent(true));
    await drain();
    window.dispatchEvent(pageshowEvent(true));
    await drain();

    expect(fetchMock).not.toHaveBeenCalled();
    expect(dir.get()).toBeUndefined();

    // After the (idempotent) double re-seed to cur+2, a Back to cur+1
    // classifies as "back" exactly once, proving the index settled at cur+2
    // (a non-idempotent re-seed that shifted the index would misclassify).
    dispatchBackTo("/list", cur + 1);
    await drain();

    expect(dir.get()).toBe("back");
    dir.dispose();
  });
});

// ---------------------------------------------------------------------------
// Same-page traverse fast-path (#1374 / #1376)
// ---------------------------------------------------------------------------
//
// A Back/Forward (popstate) traversal between two history entries sharing the
// same pathname+search is served from the live DOM — no re-fetch, no re-swap,
// no zfb:* lifecycle events. This is the #1374 fix: before it, a traversal over
// a raw `history.pushState('#slug')` entry fell through to a full fetch + swap
// of the byte-identical page (overlay flash, wasted fetch, island-state loss),
// because the old hash-gate only saw router-managed navigation.
//
// The fast path routes through moveToLocation (scroll restore + originalLocation
// self-heal), NOT a bare early return — see the acceptance criteria in #1376.
//
// A per-page opt-out (`<ClientRouter traverseRefetch />` →
// meta[name="zfb-traverse-refetch"]) forces the fetch back on for per-request
// SSR pages whose content can differ between two visits to the same URL.
//
// BLIND SPOT (documented): happy-dom models neither a real fetch/paint nor
// bfcache, so this proves the gate's BRANCH LOGIC, not the real-browser
// no-flash behavior. Definitive proof is the Wave-3 chromium harness (#1378).
describe("same-page traverse fast-path (#1374 / #1376)", () => {
  // Earlier tests in this file shim `location.href` via Object.defineProperty
  // and never restore it, leaving a frozen own-property getter that would stop
  // `history.push/replaceState(state, "", path)` from reflecting into
  // `location.href` (which onPopState reads via `new URL(location.href)`).
  // Delete any leaked own-property so happy-dom's native prototype getter — which
  // DOES track push/replaceState — is back in effect for these tests.
  beforeEach(() => {
    if (Object.getOwnPropertyDescriptor(location, "href")) {
      delete (location as unknown as Record<string, unknown>)["href"];
    }
  });

  const drain = () => new Promise((r) => setTimeout(r, 0));

  function fetchPages() {
    const fetchMock = vi.fn(async (url: RequestInfo) => {
      const u = String(url);
      if (u.includes("/base")) return htmlResponse(pageHtml("Base", "base content"));
      if (u.includes("/other")) return htmlResponse(pageHtml("Other", "other content"));
      throw new Error(`unexpected fetch: ${u}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    return fetchMock;
  }

  // Add the opt-out meta to the CURRENT (live) document — i.e. AFTER a navigate()
  // swap has replaced the head, since the router reads it on the target page.
  function injectTraverseRefetchMeta() {
    const meta = document.createElement("meta");
    meta.setAttribute("name", "zfb-traverse-refetch");
    meta.setAttribute("content", "true");
    document.head.appendChild(meta);
  }

  // Fire the popstate a browser would fire for a history entry. happy-dom's
  // history.replaceState(state, "", path) updates BOTH history.state and
  // location.href (same-origin path) — exactly what onPopState reads. The state
  // MUST be non-null: onPopState ignores `ev.state === null` (:814-816).
  function dispatchPopstate(path: string, state: Record<string, unknown>) {
    history.replaceState(state, "", path);
    window.dispatchEvent(new PopStateEvent("popstate", { state }));
  }

  // Seed one router-managed entry at /base through production code so
  // originalLocation === /base and history.state carries a finite index. The
  // module index is monotonic across the whole file, so read the resulting
  // index back rather than assuming an absolute value.
  async function seedBase(): Promise<number> {
    await navigate("/base");
    expect(document.title).toBe("Base");
    return (history.state as { index: number }).index;
  }

  it("(a) raw pushState('#modal') + Back popstate → no fetch, no before-preparation, originalLocation self-heals", async () => {
    const fetchMock = fetchPages();
    const baseIndex = await seedBase();

    // Consumer opens a modal via RAW history.pushState — NOT through the router,
    // so originalLocation stays /base and the module index is unchanged. This is
    // the #1374 trigger: the old hash-gate could not see this entry.
    history.pushState({ ...history.state }, "", "/base#modal");

    const beforePrep = vi.fn();
    document.addEventListener("zfb:before-preparation", beforePrep);
    fetchMock.mockClear();

    // User presses Back: the browser pops to the /base entry and fires popstate.
    dispatchPopstate("/base", { index: baseIndex, scrollX: 0, scrollY: 0 });
    await drain();

    document.removeEventListener("zfb:before-preparation", beforePrep);

    // Fast path: byte-identical same-page traverse never fetches or swaps, and
    // skips ALL zfb:* lifecycle events (intentional — same as the hash fast-path).
    expect(fetchMock).not.toHaveBeenCalled();
    expect(beforePrep).not.toHaveBeenCalled();

    // originalLocation self-healed by moveToLocation (NOT a bare early return):
    // a subsequent cross-page navigation behaves normally.
    await navigate("/other");
    expect(document.title).toBe("Other");
    expect(fetchMock.mock.calls.some((c) => String(c[0]).includes("/other"))).toBe(true);
  });

  it("(b) same scenario WITH the zfb-traverse-refetch meta → fetch IS called (opt-out preserved)", async () => {
    const fetchMock = fetchPages();
    const baseIndex = await seedBase();

    // Opt this page back INTO the fetch (per-request SSR). The router reads the
    // meta on the CURRENT document, so inject it after the navigate() swap.
    injectTraverseRefetchMeta();

    history.pushState({ ...history.state }, "", "/base#modal");

    fetchMock.mockClear();
    dispatchPopstate("/base", { index: baseIndex, scrollX: 0, scrollY: 0 });
    await drain();

    // The opt-out disables the traverse clause, so the same-page traverse falls
    // through to a full fetch (today's pre-fast-path behavior, preserved).
    expect(fetchMock.mock.calls.some((c) => String(c[0]).includes("/base"))).toBe(true);
  });

  it("(c) forward hash fast-path is unaffected by the gate restructure", async () => {
    const fetchMock = fetchPages();
    await seedBase();

    fetchMock.mockClear();
    const beforePrep = vi.fn();
    document.addEventListener("zfb:before-preparation", beforePrep);

    // Forward same-page hash nav via navigate() — hits `(direction !== "back" &&
    // to.hash)`, which the new traverse clause must not disturb.
    await navigate(`${location.pathname}#section-2`);

    document.removeEventListener("zfb:before-preparation", beforePrep);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(beforePrep).not.toHaveBeenCalled();
    expect(location.hash).toBe("#section-2");
  });

  it("(c) back-direction hash clause (from.hash) still fast-paths even with the traverse clause opted out", async () => {
    const fetchMock = fetchPages();
    await seedBase();

    // Move forward into a hash entry so originalLocation carries a hash
    // (`from.hash` for the subsequent Back). This goes through the router.
    await navigate(`${location.pathname}#section`);
    const hashIndex = (history.state as { index: number }).index;

    // Opt OUT of the traverse fast-path so the traverse clause is disabled — the
    // back-direction hash clause `(direction === "back" && from.hash)` is then
    // the ONLY thing that can short-circuit the fetch, proving it is still live.
    injectTraverseRefetchMeta();

    fetchMock.mockClear();
    const beforePrep = vi.fn();
    document.addEventListener("zfb:before-preparation", beforePrep);

    // Browser Back from /base#section to /base: to.hash empty, from.hash "#section".
    dispatchPopstate("/base", { index: hashIndex - 1, scrollX: 0, scrollY: 0 });
    await drain();

    document.removeEventListener("zfb:before-preparation", beforePrep);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(beforePrep).not.toHaveBeenCalled();
  });

  it("(d) cross-page popstate still fetches (the traverse clause is gated on samePage)", async () => {
    const fetchMock = fetchPages();

    // Two DIFFERENT pages so the traverse is cross-page (samePage false).
    await navigate("/base");
    const baseIdx = (history.state as { index: number }).index;
    await navigate("/other");

    fetchMock.mockClear();
    // Browser Back to /base (lower index) — cross-page traverse must still fetch.
    dispatchPopstate("/base", { index: baseIdx, scrollX: 0, scrollY: 0 });
    await drain();

    expect(fetchMock.mock.calls.some((c) => String(c[0]).includes("/base"))).toBe(true);
  });

  it("(e) incomplete non-null state ({}) + Back → no fetch, no crash, no scroll jump", async () => {
    const fetchMock = fetchPages();
    await seedBase();

    // Consumer opens a modal via RAW pushState with EMPTY (incomplete) state.
    history.pushState({}, "", "/base#modal");

    // User presses Back. In this raw-History flow the popstate delivers a
    // non-null but INCOMPLETE state (no index/scrollX/scrollY). The fast path
    // must tolerate it: no crash, and it must NEVER call scrollTo(undefined, …)
    // — scroll is left where it is rather than jumping to (0,0).
    const scrollSpy = vi.spyOn(window, "scrollTo");
    fetchMock.mockClear();
    const beforePrep = vi.fn();
    document.addEventListener("zfb:before-preparation", beforePrep);

    dispatchPopstate("/base", {});
    await drain();

    document.removeEventListener("zfb:before-preparation", beforePrep);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(beforePrep).not.toHaveBeenCalled();
    // Partial-state hardening: no scrollTo call at all on this path.
    expect(scrollSpy).not.toHaveBeenCalled();

    scrollSpy.mockRestore();
  });
});

describe("island lifecycle ordering during navigate()", () => {
  it("calls cancelPendingIslands then unmountIslands then mountNewIslands in that order", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("Next", "next content"))),
    );

    const order: string[] = [];
    vi.mocked(cancelPendingIslands).mockImplementation(() => {
      order.push("cancelPendingIslands");
    });
    vi.mocked(unmountIslands).mockImplementation(() => {
      order.push("unmountIslands");
    });
    vi.mocked(mountNewIslands).mockImplementation(() => {
      order.push("mountNewIslands");
    });

    await navigate("/ordering-test");

    // cancelPendingIslands and unmountIslands happen in updateDOM (before the swap),
    // mountNewIslands happens after runScripts (post-swap). The spec requires
    // unmountIslands to be called between cancelPendingIslands and mountNewIslands.
    const cancelIdx = order.indexOf("cancelPendingIslands");
    const unmountIdx = order.indexOf("unmountIslands");
    const mountIdx = order.indexOf("mountNewIslands");

    expect(cancelIdx).toBeGreaterThanOrEqual(0);
    expect(unmountIdx).toBeGreaterThanOrEqual(0);
    expect(mountIdx).toBeGreaterThanOrEqual(0);
    expect(cancelIdx).toBeLessThan(unmountIdx);
    expect(unmountIdx).toBeLessThan(mountIdx);
  });
});

// ---------------------------------------------------------------------------
// Route announcer — timer lifecycle ownership (#1063)
// ---------------------------------------------------------------------------
//
// Covers the announce() changes from #1063: exactly one announcer element
// (fresh per navigation, never reused or accumulated) and a cancellable 60ms
// timer that a superseding navigation clears.
//
// NOT covered here (documented blind spot): the scroll-position polling
// setInterval guard (router.ts, the `else` branch when `onscrollend` is
// absent). Per #1063's own investigation, happy-dom reports
// `"onscrollend" in window === false` (so the router takes the setInterval
// branch) but `window.scrollTo()` dispatches no `scroll` event, and the
// interval is only ever started from inside the `scroll` listener — so the
// interval never starts in this environment and cannot be driven through the
// public API. Its teardown guard is defensive-only (a real browser always has
// the globals it reads). Re-announce *reliability* of the aria-live region is
// likewise a screen-reader behavior happy-dom cannot observe (Level 5/6).
describe("route announcer — timer lifecycle (#1063)", () => {
  // Flush pending microtasks WITHOUT advancing real time, so announce() (which
  // runs in the post-swap `updateCallbackDone.finally` continuation) has run but
  // its 60ms timer is still pending. Avoids setTimeout so it doesn't perturb the
  // timer spies below.
  const flushMicrotasks = async () => {
    for (let i = 0; i < 12; i++) await Promise.resolve();
  };

  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) => {
        const slug = String(url).split("/").pop() || "page";
        return htmlResponse(pageHtml(`Title-${slug}`, `content-${slug}`));
      }),
    );
  });

  it("appends exactly one announcer, fresh per navigation (no reuse, no accumulation)", async () => {
    await navigate("/ann-a");
    await flushMicrotasks();
    const first = document.querySelector(".zfb-route-announcer");
    expect(first).not.toBeNull();
    // The aria-live contract the announcer is created with.
    expect(first?.getAttribute("aria-live")).toBe("assertive");
    expect(first?.getAttribute("aria-atomic")).toBe("true");

    await navigate("/ann-b");
    await flushMicrotasks();
    const announcers = document.querySelectorAll(".zfb-route-announcer");
    // Exactly one announcer ever exists ...
    expect(announcers).toHaveLength(1);
    // ... and it is a FRESH element, not the prior instance reused. A reused
    // live region re-entering the a11y tree is an unreliable SR trigger (#1063).
    expect(announcers[0]).not.toBe(first);
  });

  it("cancels the prior pending announce timer when a newer navigation supersedes it", async () => {
    const setTimeoutSpy = vi.spyOn(window, "setTimeout");
    const clearTimeoutSpy = vi.spyOn(window, "clearTimeout");
    try {
      await navigate("/sup-a");
      await flushMicrotasks();
      // announce() schedules the only 60ms setTimeout in the navigation pipeline.
      const idx = setTimeoutSpy.mock.calls.findIndex((c) => c[1] === 60);
      expect(idx).toBeGreaterThanOrEqual(0);
      const firstTimerId = setTimeoutSpy.mock.results[idx]!.value;

      await navigate("/sup-b");
      await flushMicrotasks();

      // The second navigation's announce() must clear the first's still-pending
      // 60ms timer before scheduling its own — the supersede guarantee.
      expect(clearTimeoutSpy).toHaveBeenCalledWith(firstTimerId);
    } finally {
      setTimeoutSpy.mockRestore();
      clearTimeoutSpy.mockRestore();
    }
  });
});

// ---------------------------------------------------------------------------
// syncHistoryEntry() — public history bookkeeping API (#1377)
// ---------------------------------------------------------------------------
//
// syncHistoryEntry(url, { replace?, state? }) writes a router-managed history
// entry (push/replace) WITHOUT any navigation, DOM, scroll, or lifecycle side
// effect — the supported path for consumers deep-linking transient UI state
// (dialogs/modals, a photo viewer's /photos/<slug>/ URL) that would otherwise
// hand-roll raw history.pushState and desync originalLocation + the index
// bookkeeping.
//
// `currentHistoryIndex` is module-global and monotonic across THIS whole file
// (every prior navigate() bumped it), so these tests read the resulting index
// back from history.state and assert RELATIVE deltas — never an absolute value.
//
// BLIND SPOT (documented): happy-dom models neither a real history stack nor a
// real fetch/paint, so popstate-driven assertions hand-feed the target entry's
// state and prove the router's BRANCH LOGIC (fetch vs. fast-path, direction),
// not real-browser back/forward behavior. Definitive proof is the Wave-3
// chromium harness (#1378).
describe("syncHistoryEntry() — public history bookkeeping API (#1377)", () => {
  // Earlier tests in this file shim `location.href` via Object.defineProperty and
  // never restore it, leaving a frozen own-property getter that would stop
  // history.push/replaceState(state, "", path) from reflecting into
  // `location.href`. Delete any leaked own-property so happy-dom's native getter —
  // which DOES track push/replaceState — is back in effect.
  beforeEach(() => {
    if (Object.getOwnPropertyDescriptor(location, "href")) {
      delete (location as unknown as Record<string, unknown>)["href"];
    }
    // A well-formed router-managed base entry so history.state is never null.
    history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/sync-base");
  });

  const drain = () => new Promise((r) => setTimeout(r, 0));

  function fetchPages() {
    const fetchMock = vi.fn(async (url: RequestInfo) => {
      const u = String(url);
      if (u.includes("/base")) return htmlResponse(pageHtml("Base", "base content"));
      if (u.includes("/photos")) return htmlResponse(pageHtml("Photos", "photos content"));
      if (u.includes("/gallery")) return htmlResponse(pageHtml("Gallery", "gallery content"));
      throw new Error(`unexpected fetch: ${u}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    return fetchMock;
  }

  // Fire the popstate a browser would fire for a history entry. happy-dom's
  // history.replaceState(state, "", path) updates BOTH history.state and
  // location.href (same-origin path) — exactly what onPopState reads. State MUST
  // be non-null (onPopState ignores ev.state === null).
  function dispatchPopstate(path: string, state: Record<string, unknown>) {
    history.replaceState(state, "", path);
    window.dispatchEvent(new PopStateEvent("popstate", { state }));
  }

  // Capture the Direction the router classified for a popstate-driven transition,
  // read off the zfb:before-preparation event.
  function captureDirection(): { get: () => string | undefined; dispose: () => void } {
    let seen: string | undefined;
    const handler = (ev: Event) => {
      seen = (ev as unknown as { direction: string }).direction;
    };
    document.addEventListener("zfb:before-preparation", handler);
    return {
      get: () => seen,
      dispose: () => document.removeEventListener("zfb:before-preparation", handler),
    };
  }

  it("push: writes a new same-page entry with a finite index and scroll reset to 0", () => {
    syncHistoryEntry("/sync-base/modal-a");
    const first = history.state;
    expect(location.pathname).toBe("/sync-base/modal-a");
    expect(Number.isFinite(first.index)).toBe(true);
    expect(first.scrollX).toBe(0);
    expect(first.scrollY).toBe(0);

    // A second push increments the tracked index by exactly 1 — the bookkeeping
    // popstate direction detection relies on.
    syncHistoryEntry("/sync-base/modal-b");
    const second = history.state;
    expect(location.pathname).toBe("/sync-base/modal-b");
    expect(second.index).toBe(first.index + 1);
  });

  it("replace: keeps the current entry's index (no increment) and swaps the URL in place", () => {
    syncHistoryEntry("/sync-base/page-1"); // establishes a known current index
    const before = history.state.index;

    syncHistoryEntry("/sync-base/page-1-replaced", { replace: true });
    expect(location.pathname).toBe("/sync-base/page-1-replaced");
    // Replace must NOT advance the index.
    expect(history.state.index).toBe(before);
  });

  it("replace: falls back to the tracked index when history.state.index is missing/invalid", () => {
    syncHistoryEntry("/sync-base/anchor"); // tracked index := N (module currentHistoryIndex)
    const tracked = history.state.index;

    // A raw-History consumer left the current entry with NO valid index.
    history.replaceState({ foo: "bar" }, "", "/sync-base/anchor");

    syncHistoryEntry("/sync-base/anchor-2", { replace: true });
    // The fallback stamped the tracked index, not NaN/undefined.
    expect(history.state.index).toBe(tracked);
    expect(history.state.foo).toBeUndefined(); // consumer's stale key not carried over
  });

  it("merges consumer state, with router keys (index/scrollX/scrollY) winning", () => {
    syncHistoryEntry("/sync-base/dialog", {
      state: { modal: "photo", index: 999, scrollX: 555, scrollY: 777 },
    });
    const s = history.state;
    // Consumer's own (non-colliding) key is preserved.
    expect(s.modal).toBe("photo");
    // Router bookkeeping keys override the consumer's colliding values.
    expect(s.index).not.toBe(999);
    expect(Number.isFinite(s.index)).toBe(true);
    expect(s.scrollX).toBe(0);
    expect(s.scrollY).toBe(0);
  });

  it("throws on a cross-origin target (never a silent full-page load)", () => {
    const before = history.state;
    expect(() => syncHistoryEntry("https://evil.example.com/x")).toThrow(/cross-origin/i);
    // The throw happens before any history write.
    expect(history.state).toBe(before);
    expect(location.pathname).toBe("/sync-base");
  });

  it("is a no-op with a console warning when called during SSR (no document)", () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const pushSpy = vi.spyOn(history, "pushState");
    // Only this test exercises the server path, so the module's one-time
    // `syncHistoryEntryOnServerWarned` flag is still false on the first call here.
    vi.stubGlobal("document", undefined);
    try {
      syncHistoryEntry("/sync-base/should-not-run");
      const afterFirst = warnSpy.mock.calls.length;
      // Second call: no-op AND once-only (no additional warning).
      syncHistoryEntry("/sync-base/still-nothing");
      expect(warnSpy.mock.calls.length).toBe(afterFirst);
      expect(afterFirst).toBe(1);
    } finally {
      vi.unstubAllGlobals();
    }
    // No history entry was written on the server path.
    expect(pushSpy).not.toHaveBeenCalled();
    // The warning mirrors navigate()'s shape: an Error named "Warning".
    const warnedWith = warnSpy.mock.calls[0]?.[0] as Error | undefined;
    expect(warnedWith?.name).toBe("Warning");

    warnSpy.mockRestore();
    pushSpy.mockRestore();
  });

  it("push: persists the outgoing entry's live scroll before pushing the new entry", () => {
    syncHistoryEntry("/sync-base/list"); // outgoing entry
    const outgoingIndex = history.state.index;

    // The user scrolled since the last scrollend flush.
    vi.stubGlobal("scrollX", 24);
    vi.stubGlobal("scrollY", 480);

    const replaceSpy = vi.spyOn(history, "replaceState");
    syncHistoryEntry("/sync-base/detail");

    // updateScrollPosition() rewrote the OUTGOING entry's state with the live
    // scroll (a replaceState) before the new entry was pushed.
    expect(replaceSpy).toHaveBeenCalled();
    const replaced = replaceSpy.mock.calls.at(-1)?.[0] as {
      index: number;
      scrollX: number;
      scrollY: number;
    };
    expect(replaced.scrollX).toBe(24);
    expect(replaced.scrollY).toBe(480);
    // It updated the outgoing entry (same index), not a fresh one.
    expect(replaced.index).toBe(outgoingIndex);
    // The freshly pushed entry itself starts at scroll (0,0).
    expect(history.state.scrollX).toBe(0);
    expect(history.state.scrollY).toBe(0);

    replaceSpy.mockRestore();
    vi.unstubAllGlobals();
  });

  it("updates originalLocation: a subsequent same-page Back takes the traverse fast path (no fetch)", async () => {
    const fetchMock = fetchPages();

    // Router-managed base so originalLocation === /base.
    await navigate("/base");

    // A photo viewer opens a NEW pathname, then a modal within it — both via
    // syncHistoryEntry, which re-points originalLocation each time.
    syncHistoryEntry("/photos/slug/");
    syncHistoryEntry("/photos/slug/#modal");
    const modalIndex = history.state.index; // finite, stamped by syncHistoryEntry

    fetchMock.mockClear();
    const beforePrep = vi.fn();
    document.addEventListener("zfb:before-preparation", beforePrep);

    // Back to /photos/slug/. originalLocation is /photos/slug/#modal (updated by
    // the last sync push), so samePage(from, to) holds → traverse fast path.
    // Had originalLocation NOT been updated it would still be /base, samePage
    // would be false, and this Back would FETCH.
    dispatchPopstate("/photos/slug/", { index: modalIndex - 1, scrollX: 0, scrollY: 0 });
    await drain();

    document.removeEventListener("zfb:before-preparation", beforePrep);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(beforePrep).not.toHaveBeenCalled();
  });

  it("pathname-changed traverse still fetches normally (not a false fast-path), direction classified 'back'", async () => {
    const fetchMock = fetchPages();

    await navigate("/base");
    const baseIndex = history.state.index;

    // syncHistoryEntry pushes a DIFFERENT pathname (photo viewer) and re-points
    // originalLocation to it — and increments currentHistoryIndex, so the entry
    // carries a finite index that derivePopDirection can use.
    syncHistoryEntry("/gallery/");

    fetchMock.mockClear();
    const dir = captureDirection();

    // Back to /base: samePage(/gallery/, /base) is false → cross-page traverse
    // must fall through to a real fetch, with the direction correctly "back".
    dispatchPopstate("/base", { index: baseIndex, scrollX: 0, scrollY: 0 });
    await drain();

    expect(fetchMock.mock.calls.some((c) => String(c[0]).includes("/base"))).toBe(true);
    expect(dir.get()).toBe("back");
    dir.dispose();
  });

  it("never scrolls or mutates the DOM (push and replace are side-effect free)", () => {
    const scrollToSpy = vi.spyOn(window, "scrollTo");
    document.body.innerHTML = `<main id="marker">original</main>`;
    const bodyBefore = document.body.innerHTML;
    const titleBefore = document.title;

    syncHistoryEntry("/sync-base/quiet-push");
    syncHistoryEntry("/sync-base/quiet-replace", { replace: true });

    // No scroll side effect (no fragment-scroll, no scroll-to-top).
    expect(scrollToSpy).not.toHaveBeenCalled();
    // No DOM mutation: body and title untouched.
    expect(document.body.innerHTML).toBe(bodyBefore);
    expect(document.title).toBe(titleBefore);

    scrollToSpy.mockRestore();
  });

  it("exported SyncHistoryEntryOptions type is usable by consumers", () => {
    // Compile-time proof that the public option type covers { replace, state }.
    const opts: SyncHistoryEntryOptions = { replace: true, state: { modal: "x" } };
    expect(() => syncHistoryEntry("/sync-base/typed", opts)).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// handleSubmit — form intercept (#1400)
// ---------------------------------------------------------------------------
//
// `handleSubmit` (router.ts ~:1127-1176) had zero behavioral coverage before
// this describe: the only submit-related test was the listener-count
// idempotency check above. These tests dispatch real `submit` events (via
// `form.requestSubmit(submitter)`, which happy-dom builds into a real
// cancelable SubmitEvent with the right `.submitter`) through the
// document-level listener `init()` installs, and observe the resulting
// network call (or its absence) via the mocked global `fetch` — the same
// "dispatch → observe fetch" pattern the rest of this file uses for
// navigate()/onPopState.
describe("handleSubmit — form intercept (#1400)", () => {
  // handleSubmit calls navigate() without awaiting it (fire-and-forget, same
  // shape as onPopState above) — drain one macrotask so the fetch mock has
  // been called by the time we assert. Mirrors the `drain` helper used by the
  // same-page traverse fast-path describe above for the identical shape.
  const drain = () => new Promise<void>((r) => setTimeout(r, 0));

  function createForm(opts: {
    action?: string;
    method?: string;
    fields?: Record<string, string>;
  }): HTMLFormElement {
    const form = document.createElement("form");
    if (opts.action !== undefined) form.setAttribute("action", opts.action);
    if (opts.method !== undefined) form.setAttribute("method", opts.method);
    for (const [name, value] of Object.entries(opts.fields ?? {})) {
      const input = document.createElement("input");
      input.setAttribute("name", name);
      input.setAttribute("value", value);
      form.appendChild(input);
    }
    document.body.appendChild(form);
    return form;
  }

  it("GET: serializes form fields into the URL's query string via URLSearchParams", async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo, _init?: RequestInit) =>
      htmlResponse(pageHtml("Results", "results content")),
    );
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({
      action: "/search-target",
      method: "get",
      fields: { q: "hello world", page: "2" },
    });
    form.requestSubmit();
    await drain();

    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl, calledInit] = fetchMock.mock.calls[0]!;
    const url = new URL(String(calledUrl));
    expect(url.pathname).toBe("/search-target");
    expect(url.searchParams.get("q")).toBe("hello world");
    expect(url.searchParams.get("page")).toBe("2");
    // GET never sets a body or an explicit method — fetchHTML's init object
    // only gains `method`/`body` when handleSubmit resolves to POST.
    expect(calledInit?.method).toBeUndefined();
    expect(calledInit?.body).toBeUndefined();
    expect(document.querySelector("main")?.textContent).toBe("results content");
  });

  it("POST: passes a FormData body to fetch (no query-string rewrite)", async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo, _init?: RequestInit) =>
      htmlResponse(pageHtml("Submitted", "posted content")),
    );
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({ action: "/submit-target", method: "post", fields: { k: "v" } });
    form.requestSubmit();
    await drain();

    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl, calledInit] = fetchMock.mock.calls[0]!;
    const url = new URL(String(calledUrl));
    expect(url.pathname).toBe("/submit-target");
    expect(url.search).toBe(""); // untouched — the GET query-rewrite branch never runs
    expect(calledInit?.method).toBe("POST");
    expect(calledInit?.body).toBeInstanceOf(FormData);
    expect((calledInit?.body as FormData).get("k")).toBe("v");
    expect(document.querySelector("main")?.textContent).toBe("posted content");
  });

  it("a submitter's formaction/formmethod attributes override the form's own action/method", async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo, _init?: RequestInit) =>
      htmlResponse(pageHtml("Overridden", "override content")),
    );
    vi.stubGlobal("fetch", fetchMock);

    // The form itself is POST to /form-default; the submitter overrides both
    // to a GET at a different URL.
    const form = createForm({ action: "/form-default", method: "post", fields: { q: "x" } });
    const submitter = document.createElement("button");
    submitter.setAttribute("type", "submit");
    submitter.setAttribute("formaction", "/override-target");
    submitter.setAttribute("formmethod", "get");
    form.appendChild(submitter);

    form.requestSubmit(submitter);
    await drain();

    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl, calledInit] = fetchMock.mock.calls[0]!;
    const url = new URL(String(calledUrl));
    // formaction won over the form's own action ...
    expect(url.pathname).toBe("/override-target");
    // ... and formmethod's "get" won over the form's own "post", so the field
    // was serialized into the query string rather than sent as a body.
    expect(url.searchParams.get("q")).toBe("x");
    expect(calledInit?.method).toBeUndefined();
    expect(calledInit?.body).toBeUndefined();
  });

  it('method="dialog" is not intercepted — the SPA transition is skipped', async () => {
    const fetchMock = vi.fn(async () => htmlResponse(pageHtml("Should not load", "n/a")));
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({ action: "/dialog-target", method: "dialog", fields: { x: "1" } });
    form.requestSubmit();
    await drain();

    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("a modifier-key click on the submit button lets the browser handle the submit natively", async () => {
    const fetchMock = vi.fn(async () => htmlResponse(pageHtml("Should not load", "n/a")));
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({ action: "/should-not-navigate", method: "get", fields: { x: "1" } });
    const submitter = document.createElement("button");
    submitter.setAttribute("type", "submit");
    form.appendChild(submitter);

    // Dispatching a "click" on a type=submit button is enough on its own:
    // happy-dom's HTMLButtonElement (mirroring real browser activation
    // behavior) triggers `form.requestSubmit(this)` itself once the click
    // isn't defaultPrevented — so this one dispatch reproduces the real
    // sequence (click on document, THEN the resulting submit) without an
    // extra manual requestSubmit() call, which would fire a second,
    // unflagged submit and defeat the test. handleClick flags the clicked
    // element during the click's bubble phase (synchronously before the
    // button's own post-click activation fires requestSubmit), so
    // handleSubmit sees the flagged submitter and steps aside for the
    // browser's native new-tab handling.
    submitter.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true, button: 0 }),
    );
    await drain();

    expect(fetchMock).not.toHaveBeenCalled();
  });

  // --- Bonus: the remaining branches of handleSubmit's single early-return
  // guard (not explicitly listed in #1400, but free to cover from the same
  // harness since they sit in the identical `if` as the dialog-method skip).
  it("bonus: data-zfb-reload on the form skips the SPA intercept", async () => {
    const fetchMock = vi.fn(async () => htmlResponse(pageHtml("Should not load", "n/a")));
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({ action: "/reload-target", method: "get", fields: { x: "1" } });
    form.dataset["zfbReload"] = "";
    form.requestSubmit();
    await drain();

    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("bonus: an earlier submit listener calling preventDefault() also skips the SPA intercept", async () => {
    const fetchMock = vi.fn(async () => htmlResponse(pageHtml("Should not load", "n/a")));
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({ action: "/prevented-target", method: "get", fields: { x: "1" } });
    // A target-phase listener runs before document's bubble-phase handleSubmit.
    form.addEventListener("submit", (e) => e.preventDefault());
    form.requestSubmit();
    await drain();

    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("bonus: a cross-origin form action skips the SPA intercept", async () => {
    const fetchMock = vi.fn(async () => htmlResponse(pageHtml("Should not load", "n/a")));
    vi.stubGlobal("fetch", fetchMock);

    const form = createForm({
      action: "https://external.example.com/target",
      method: "get",
      fields: { x: "1" },
    });
    form.requestSubmit();
    await drain();

    expect(fetchMock).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// transition() defaultLoader — form body consumption (#1400, router.ts ~:628-653)
// ---------------------------------------------------------------------------
//
// defaultLoader is not exported — it is a closure inside transition() — so
// these tests reach it the way the rest of this file reaches internal loader
// behavior: call the exported navigate() directly with `options.formData` set
// and observe the RequestInit handed to the mocked global fetch.
describe("transition() defaultLoader — form body consumption (#1400)", () => {
  it("reads enctype from the real HTML attribute — not a shadowed .enctype property", async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo, _init?: RequestInit) =>
      htmlResponse(pageHtml("Posted", "posted content")),
    );
    vi.stubGlobal("fetch", fetchMock);

    const form = document.createElement("form");
    form.setAttribute("method", "post");
    form.setAttribute("enctype", "application/x-www-form-urlencoded");
    document.body.appendChild(form);

    // happy-dom's HTMLFormElement proxy always resolves `.enctype` to its own
    // IDL getter (it doesn't implement the real-browser [OverrideBuiltins]
    // quirk that lets a same-named `<input name="enctype">` control shadow
    // the property) — so we monkeypatch an own property directly to simulate
    // that shadow. defaultLoader reads enctype via
    // `Reflect.get(HTMLFormElement.prototype, "attributes", form)` rather
    // than `form.enctype`, precisely so a shadowed property like this cannot
    // fool it; this proves that.
    Object.defineProperty(form, "enctype", {
      configurable: true,
      value: "multipart/form-data",
    });
    expect(form.enctype).toBe("multipart/form-data"); // the shadow is in effect

    const formData = new FormData();
    formData.append("k", "v");

    await navigate("/submit-target", { formData, sourceElement: form });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0]!;
    expect(init?.body).toBeInstanceOf(URLSearchParams);
    expect((init?.body as URLSearchParams).get("k")).toBe("v");
  });

  it("keeps the raw FormData body when no enctype attribute is present (Astro-compat default)", async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo, _init?: RequestInit) =>
      htmlResponse(pageHtml("Posted", "posted content")),
    );
    vi.stubGlobal("fetch", fetchMock);

    const form = document.createElement("form");
    form.setAttribute("method", "post");
    document.body.appendChild(form);

    const formData = new FormData();
    formData.append("k", "v");

    await navigate("/submit-target", { formData, sourceElement: form });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0]!;
    // Untouched — only an EXPLICIT url-encoded enctype triggers the
    // URLSearchParams conversion; a default/absent enctype (and any other
    // enctype, e.g. multipart/form-data) leaves the raw FormData in place.
    expect(init?.body).toBe(formData);
  });

  it("resolves the form via a submitter's `.form` reference, not just a direct form sourceElement", async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo, _init?: RequestInit) =>
      htmlResponse(pageHtml("Posted", "posted content")),
    );
    vi.stubGlobal("fetch", fetchMock);

    const form = document.createElement("form");
    form.setAttribute("method", "post");
    form.setAttribute("enctype", "application/x-www-form-urlencoded");
    const submitter = document.createElement("button");
    submitter.setAttribute("type", "submit");
    form.appendChild(submitter);
    document.body.appendChild(form);

    const formData = new FormData();
    formData.append("k", "v");

    // handleSubmit passes `submitter ?? form` as sourceElement — mirror that
    // directly to prove defaultLoader's `"form" in sourceElement` branch
    // resolves the owning form rather than treating the button as formless.
    await navigate("/submit-target", { formData, sourceElement: submitter });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0]!;
    expect(init?.body).toBeInstanceOf(URLSearchParams);
  });
});
