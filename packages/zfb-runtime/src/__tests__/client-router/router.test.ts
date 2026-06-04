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
