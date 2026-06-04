/**
 * @vitest-environment happy-dom
 */
// Unit tests for client-router/prefetch.
//
// happy-dom sets location.origin to http://localhost:3000 — all same-origin
// test URLs must use that origin. Cross-origin tests use https://external.example.com.
//
// Coverage:
//   1. hover trigger: pointerenter issues a prefetch after idle delay
//   2. hover cancel: pointerleave within the delay cancels the prefetch
//   3. viewport trigger: IntersectionObserver fires isIntersecting:true → prefetch issued
//   4. load trigger: requestIdleCallback queues prefetch for data-zfb-prefetch="load" links
//   5. tap trigger: mousedown on a link issues a prefetch immediately
//   6. cross-origin skip: cross-origin hrefs are never prefetched
//   7. in-flight dedup: two concurrent triggers for the same href produce one network call
//   8. per-link disable: data-zfb-prefetch="false" is never prefetched
//   9. disabled-flag: meta[name="zfb-prefetch-disabled"][content="true"] makes init() a no-op
//   10. post-swap re-scan: zfb:after-swap causes newly-inserted viewport links to be observed
//   11. prefetchAll multi-mount: calling init twice doesn't double-register listeners
//   12. ClientRouter without prefetchAll does NOT call prefetchInit
//   13. link method: <link rel=prefetch> used when supported

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

// ---------------------------------------------------------------------------
// Stubs — installed BEFORE module import so module evaluates with them in scope
// ---------------------------------------------------------------------------

// requestIdleCallback stub — fires synchronously for deterministic tests.
type IdleCallback = (deadline: { didTimeout: boolean; timeRemaining: () => number }) => void;

// Use deferred mode by default; tests that need synchronous firing override per-test.
const deferredCallbacks = new Map<number, IdleCallback>();
let idleHandle = 0;

globalThis.requestIdleCallback = (cb: IdleCallback): number => {
  const h = ++idleHandle;
  // Fire synchronously by default.
  cb({ didTimeout: false, timeRemaining: () => 50 });
  return h;
};
globalThis.cancelIdleCallback = (_h: number): void => {
  deferredCallbacks.delete(_h);
};

// IntersectionObserver stub — exposes `fire(el, isIntersecting)` helper.
type IoCallback = (entries: IntersectionObserverEntry[]) => void;

interface StubIo extends IntersectionObserver {
  fire(el: Element, isIntersecting: boolean): void;
}

let lastIo: StubIo | null = null;

class StubIntersectionObserver implements StubIo {
  readonly root = null;
  readonly rootMargin = "";
  readonly thresholds: readonly number[] = [0];

  private _callback: IoCallback;
  private _observed = new Set<Element>();

  constructor(cb: IoCallback) {
    this._callback = cb;
    lastIo = this;
  }

  observe(el: Element): void {
    this._observed.add(el);
  }

  unobserve(el: Element): void {
    this._observed.delete(el);
  }

  disconnect(): void {
    this._observed.clear();
    if (lastIo === this) lastIo = null;
  }

  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }

  fire(el: Element, isIntersecting: boolean): void {
    this._callback([
      {
        isIntersecting,
        target: el,
        intersectionRatio: isIntersecting ? 1 : 0,
        boundingClientRect: {} as DOMRectReadOnly,
        intersectionRect: {} as DOMRectReadOnly,
        rootBounds: null,
        time: 0,
      } as unknown as IntersectionObserverEntry,
    ]);
  }

  hasObserved(el: Element): boolean {
    return this._observed.has(el);
  }
}

(
  globalThis as unknown as { IntersectionObserver: typeof StubIntersectionObserver }
).IntersectionObserver = StubIntersectionObserver;

// fetch stub — installed before import so the module captures it from globalThis at call time.
const fetchMock = vi.fn().mockResolvedValue(new Response());
(globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock;

// ---------------------------------------------------------------------------
// Import SUT after stubs
// ---------------------------------------------------------------------------

import { __resetForTests, init, prefetch } from "../../client-router/prefetch.js";

// ---------------------------------------------------------------------------
// Test setup / helpers
// ---------------------------------------------------------------------------

// happy-dom default origin — must match location.origin in the test environment.
const ORIGIN = "http://localhost:3000";

function sameOriginUrl(path: string): string {
  return `${ORIGIN}${path}`;
}

beforeEach(() => {
  resetDocument();
  __resetForTests();
  lastIo = null;
  fetchMock.mockClear();
  // Restore synchronous requestIdleCallback (some tests override it).
  globalThis.requestIdleCallback = (cb: IdleCallback): number => {
    const h = ++idleHandle;
    cb({ didTimeout: false, timeRemaining: () => 50 });
    return h;
  };
  globalThis.cancelIdleCallback = (_h: number): void => {};
  // Default: <link rel="prefetch"> NOT supported → force fetch() path.
  stubLinkPrefetchSupport(false);
});

afterEach(drainHappyDom);

/** Control whether browser reports support for <link rel=prefetch>. */
function stubLinkPrefetchSupport(supported: boolean): void {
  Object.defineProperty(HTMLLinkElement.prototype, "relList", {
    configurable: true,
    get() {
      return { supports: (_feature: string) => supported };
    },
  });
}

/** Create an <a> link in document.body. */
function createLink(href: string, attr?: string): HTMLAnchorElement {
  const a = document.createElement("a");
  a.href = href;
  if (attr !== undefined) a.setAttribute("data-zfb-prefetch", attr);
  document.body.appendChild(a);
  return a;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("hover trigger", () => {
  it("1. pointerenter on a same-origin link issues a prefetch after idle delay", async () => {
    init();
    // Explicit data-zfb-prefetch="hover" opts this link into the hover strategy.
    const link = createLink(sameOriginUrl("/page-a"), "hover");

    link.dispatchEvent(new Event("pointerenter", { bubbles: true }));

    // requestIdleCallback fires synchronously in our stub, so the prefetch
    // function was called synchronously. Await the async fetch resolution.
    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]![0]).toBe(sameOriginUrl("/page-a"));
  });

  it("2. pointerleave within the delay cancels the prefetch", () => {
    // Switch to a deferred requestIdleCallback so we can cancel before it fires.
    const deferred = new Map<number, IdleCallback>();
    let h = 0;
    globalThis.requestIdleCallback = (cb: IdleCallback): number => {
      const id = ++h;
      deferred.set(id, cb);
      return id; // NOT fired yet
    };
    globalThis.cancelIdleCallback = (id: number): void => {
      deferred.delete(id);
    };

    init();
    // Explicit data-zfb-prefetch="hover" opts this link into the hover strategy.
    const link = createLink(sameOriginUrl("/page-b"), "hover");

    link.dispatchEvent(new Event("pointerenter", { bubbles: true }));
    expect(deferred.size).toBe(1);

    // Simulate pointerleave before idle fires.
    link.dispatchEvent(new Event("pointerleave", { bubbles: true }));
    expect(deferred.size).toBe(0);

    // Drain deferred — nothing should fire a prefetch now.
    deferred.forEach((cb) => cb({ didTimeout: false, timeRemaining: () => 50 }));
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("viewport trigger", () => {
  it("3. IntersectionObserver isIntersecting:true → prefetch issued", async () => {
    const link = createLink(sameOriginUrl("/page-viewport"), "viewport");
    init();

    expect(lastIo).not.toBeNull();
    lastIo!.fire(link, true);

    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]![0]).toBe(sameOriginUrl("/page-viewport"));
  });
});

describe("load trigger", () => {
  it("4. requestIdleCallback queues prefetch for data-zfb-prefetch=load links", async () => {
    // Create the link BEFORE init so the initial scan finds it.
    createLink(sameOriginUrl("/page-load"), "load");
    init();

    // requestIdleCallback stub fires synchronously → prefetch() called.
    // Await the async fetch resolution.
    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]![0]).toBe(sameOriginUrl("/page-load"));
  });
});

describe("tap trigger", () => {
  it("5. mousedown on a link with data-zfb-prefetch=tap issues a prefetch", async () => {
    init();
    const link = createLink(sameOriginUrl("/page-tap"), "tap");

    link.dispatchEvent(new Event("mousedown", { bubbles: true }));

    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]![0]).toBe(sameOriginUrl("/page-tap"));
  });
});

describe("cross-origin skip", () => {
  it("6. cross-origin hrefs are never prefetched", async () => {
    init();
    prefetch("https://external.example.com/page");
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("in-flight dedup", () => {
  it("7. two calls for the same href produce one network request", async () => {
    init();
    prefetch(sameOriginUrl("/dedup-page"));
    prefetch(sameOriginUrl("/dedup-page"));
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

describe("per-link disable", () => {
  it("8. data-zfb-prefetch=false is never prefetched even when prefetchAll is true", async () => {
    init({ prefetchAll: true });
    const link = createLink(sameOriginUrl("/disabled-link"), "false");

    link.dispatchEvent(new Event("pointerenter", { bubbles: true }));
    await Promise.resolve();

    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("disabled-flag", () => {
  it("9. meta[name=zfb-prefetch-disabled][content=true] makes init() a no-op", async () => {
    // Insert the disabled meta tag with the exact locked selector.
    const meta = document.createElement("meta");
    meta.setAttribute("name", "zfb-prefetch-disabled");
    meta.setAttribute("content", "true");
    document.head.appendChild(meta);

    init({ prefetchAll: true });

    // No listeners should be registered.
    const link = createLink(sameOriginUrl("/some-link"));
    link.dispatchEvent(new Event("pointerenter", { bubbles: true }));
    await Promise.resolve();

    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("post-swap re-scan", () => {
  it("10. zfb:after-swap causes newly-inserted viewport links to be observed and prefetched", async () => {
    init();
    expect(lastIo).not.toBeNull();

    // Insert a link AFTER init — missed by the initial scan.
    const newLink = createLink(sameOriginUrl("/post-swap-page"), "viewport");

    // Dispatch after-swap: the handler re-walks and observes the new link.
    document.dispatchEvent(new Event("zfb:after-swap"));

    // The new link should now be observed.
    expect((lastIo as StubIntersectionObserver).hasObserved(newLink)).toBe(true);

    // Simulate observer firing for the new link.
    lastIo!.fire(newLink, true);
    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]![0]).toBe(sameOriginUrl("/post-swap-page"));
  });
});

describe("idempotency", () => {
  it("11. calling init() twice does not double-register listeners (one prefetch issued)", async () => {
    init({ prefetchAll: true });
    init({ prefetchAll: true }); // second call is a no-op

    const link = createLink(sameOriginUrl("/pa-link"));
    link.dispatchEvent(new Event("pointerenter", { bubbles: true }));
    await Promise.resolve();

    // If listeners were double-registered, the prefetch would fire twice.
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("12. hover prefetch not issued when link has no data-zfb-prefetch and prefetchAll is false", async () => {
    init(); // prefetchAll defaults to false
    const link = createLink(sameOriginUrl("/no-attr-link")); // no data-zfb-prefetch

    link.dispatchEvent(new Event("pointerenter", { bubbles: true }));
    await Promise.resolve();

    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("link prefetch method", () => {
  it("13. uses <link rel=prefetch> when feature is supported", async () => {
    stubLinkPrefetchSupport(true);
    init();
    prefetch(sameOriginUrl("/link-method"));
    await Promise.resolve();

    const links = document.head.querySelectorAll('link[rel="prefetch"]');
    expect(links.length).toBe(1);
    expect((links[0] as HTMLLinkElement).href).toBe(sameOriginUrl("/link-method"));
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("fetch failure handling", () => {
  it("14. fetch failure does not permanently mark href as prefetched (retry is possible)", async () => {
    init();
    const url = sameOriginUrl("/retry-page");

    // First call: fetch rejects.
    fetchMock.mockRejectedValueOnce(new Error("network error"));
    prefetch(url);
    // Flush until the rejection settles and inFlight.delete runs.
    await new Promise((r) => setTimeout(r, 0));

    // Second call: fetch resolves.
    fetchMock.mockResolvedValueOnce(new Response());
    prefetch(url);
    await new Promise((r) => setTimeout(r, 0));

    // Both calls should have reached the network.
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("15. fetch failure: no unhandled rejection leaks (call site catch suppresses)", async () => {
    init();
    const url = sameOriginUrl("/no-leak-page");

    const unhandledSpy = vi.fn();
    process.on("unhandledRejection", unhandledSpy);
    try {
      fetchMock.mockRejectedValueOnce(new Error("network error"));
      prefetch(url);
      // Use a macrotask wait so that any unhandled rejection event would have fired.
      await new Promise((r) => setTimeout(r, 0));
      expect(unhandledSpy).not.toHaveBeenCalled();
    } finally {
      process.off("unhandledRejection", unhandledSpy);
    }
  });
});
