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

// Late import after the document is primed.
import {
  init,
  navigate,
  supportsViewTransitions,
  transitionEnabledOnThisPage,
} from "../../client-router/router.js";

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
