/**
 * @vitest-environment happy-dom
 */
// Regression tests for the WebKit "single Back falls off the site" bug
// (zudolab/zzmod#662): history.pushState() issued *inside* the
// document.startViewTransition() update callback is not reliably committed by
// WebKit, so no distinct back/forward entry is created. The router commits the
// SPA history entry BEFORE starting the View Transition — these tests pin that
// the write happens outside the transition's update callback.
//
// CRITICAL SEAM: `supportsViewTransitions` is captured ONCE at router
// module-eval (`inBrowser && !!document.startViewTransition`). Defining
// startViewTransition *after* importing the router would not flip it, and the
// router would silently take the fallback path — exercising the wrong branch.
// So this dedicated spec defines document.startViewTransition BEFORE the late
// router import (mirroring how router.test.ts primes the opt-in meta first).

import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

import {
  captureDocumentLoads,
  drainHappyDom,
  installHappyDomShim,
  resetDocument,
} from "./_helpers.js";
import { cancelPendingIslands, mountNewIslands, unmountIslands } from "@takazudo/zfb/runtime";

vi.mock("@takazudo/zfb/runtime", () => ({
  mountNewIslands: vi.fn(),
  cancelPendingIslands: vi.fn(),
  unmountIslands: vi.fn(),
}));

// `supportsViewTransitions` is captured at router module-eval as
// `!!document.startViewTransition`. ES imports are hoisted above ordinary
// top-level statements, so a plain pre-import assignment runs too late. vi.hoisted
// runs BEFORE the hoisted router import — define startViewTransition (and the
// animate() stubs the VT path may touch) here so the module captures it as true.
vi.hoisted(() => {
  if (typeof document === "undefined") return;
  if (typeof document.startViewTransition !== "function") {
    Object.defineProperty(document, "startViewTransition", {
      configurable: true,
      writable: true,
      // Minimal truthy stub for module-eval; beforeEach installs the real fake.
      value: (cb: () => Promise<void> | void) => {
        const done = Promise.resolve().then(() => cb());
        return {
          updateCallbackDone: done,
          ready: done,
          finished: done,
          skipTransition() {},
          types: new Set(),
        };
      },
    });
  }
  if (typeof document.getAnimations !== "function") {
    Object.defineProperty(document, "getAnimations", { configurable: true, value: () => [] });
  }
  const g = globalThis as { KeyframeEffect?: unknown };
  if (typeof g.KeyframeEffect === "undefined") g.KeyframeEffect = class KeyframeEffect {};
});

installHappyDomShim();

function enableTransitions(): void {
  const meta = document.createElement("meta");
  meta.setAttribute("name", "zfb-view-transitions-enabled");
  meta.setAttribute("content", "true");
  document.head.appendChild(meta);
}

// The per-test recorder replaces document.startViewTransition with an
// order-logging variant; the router reads it live at call time, so swapping it
// per test is sufficient.
type VTCallback = () => Promise<void> | void;
interface FakeViewTransition {
  updateCallbackDone: Promise<void>;
  ready: Promise<void>;
  finished: Promise<void>;
  skipTransition: () => void;
  types: Set<string>;
}
function makeFakeStartViewTransition(onCall?: () => void) {
  return (cb: VTCallback): FakeViewTransition => {
    onCall?.();
    // Run the update callback on a microtask, like the browser, and resolve
    // updateCallbackDone when it settles (transition() awaits this).
    const updateCallbackDone = Promise.resolve().then(() => cb()) as Promise<void>;
    return {
      updateCallbackDone,
      ready: updateCallbackDone,
      finished: updateCallbackDone,
      skipTransition: () => {},
      types: new Set<string>(),
    };
  };
}

// Late import after vi.hoisted has primed document.startViewTransition.
import { init, navigate, supportsViewTransitions } from "../../client-router/router.js";

beforeEach(() => {
  resetDocument();
  enableTransitions();
  // Install a fresh fake VT for each test (default recorder; tests that need
  // ordering swap in their own).
  (document as unknown as { startViewTransition: unknown }).startViewTransition =
    makeFakeStartViewTransition();
  // Reset location + a managed history entry to a known root so each test's
  // navigate target differs from the current URL (the commit-before guard skips
  // a write when to.href === location.href). resetDocument does not touch
  // history/location.
  history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/");
});

afterEach(async () => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  (document as unknown as { startViewTransition: unknown }).startViewTransition =
    makeFakeStartViewTransition();
  await drainHappyDom();
});

function htmlResponse(body: string): Response {
  return new Response(body, { headers: { "content-type": "text/html; charset=utf-8" } });
}
function pageHtml(title: string, mainContent: string): string {
  return `<!doctype html><html><head>
    <meta name="zfb-view-transitions-enabled" content="true">
    <title>${title}</title>
  </head><body><main>${mainContent}</main></body></html>`;
}

describe("VT-path precondition", () => {
  it("supportsViewTransitions is true (startViewTransition defined before import)", () => {
    // Without this, every assertion below would silently test the fallback path.
    expect(supportsViewTransitions).toBe(true);
  });
});

describe("native ViewTransition.ready cancellation", () => {
  it.each(["AbortError", "InvalidStateError"])(
    "observes a %s ready rejection while completing the DOM and history update",
    async (name) => {
      vi.stubGlobal(
        "fetch",
        vi.fn(async () => htmlResponse(pageHtml("B", "page b"))),
      );
      (document as unknown as { startViewTransition: unknown }).startViewTransition = (
        callback: VTCallback,
      ): FakeViewTransition => {
        const updateCallbackDone = Promise.resolve().then(callback) as Promise<void>;
        return {
          updateCallbackDone,
          ready: Promise.reject(new DOMException("Transition was skipped", name)),
          finished: updateCallbackDone,
          skipTransition: () => {},
          types: new Set<string>(),
        };
      };

      await navigate("/ready-skipped");

      expect(location.pathname).toBe("/ready-skipped");
      expect(Number.isInteger(history.state.index)).toBe(true);
      expect(document.querySelector("main")?.textContent).toBe("page b");
      // Vitest also fails this test if the independent ready promise rejects
      // without a rejection handler during the following event-loop turn.
      await new Promise((resolve) => setTimeout(resolve, 0));
    },
  );

  it("observes ready when a newer navigation skips the previous transition", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) =>
        htmlResponse(pageHtml("Page", `content for ${new URL(String(url)).pathname}`)),
      ),
    );

    let rejectFirstReady!: (reason: Error) => void;
    let finishFirst!: () => void;
    let skips = 0;
    let calls = 0;
    (document as unknown as { startViewTransition: unknown }).startViewTransition = (
      callback: VTCallback,
    ): FakeViewTransition => {
      calls++;
      const updateCallbackDone = Promise.resolve().then(callback) as Promise<void>;
      if (calls > 1) {
        return {
          updateCallbackDone,
          ready: updateCallbackDone,
          finished: updateCallbackDone,
          skipTransition: () => {},
          types: new Set<string>(),
        };
      }
      const ready = new Promise<void>((_resolve, reject) => {
        rejectFirstReady = reject;
      });
      const finished = new Promise<void>((resolve) => {
        finishFirst = resolve;
      });
      return {
        updateCallbackDone,
        ready,
        finished,
        skipTransition: () => {
          skips++;
          rejectFirstReady(new DOMException("Transition was skipped", "AbortError"));
          finishFirst();
        },
        types: new Set<string>(),
      };
    };

    await navigate("/rapid-a");
    const firstIndex = history.state.index;
    await navigate("/rapid-b");

    expect(skips).toBe(1);
    expect(location.pathname).toBe("/rapid-b");
    expect(history.state.index).toBe(firstIndex + 1);
    expect(document.querySelector("main")?.textContent).toBe("content for /rapid-b");
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  it("still reports a real update callback error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("B", "page b"))),
    );
    const error = new Error("update failed");
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    (document as unknown as { startViewTransition: unknown }).startViewTransition =
      (): FakeViewTransition => {
        const updateCallbackDone = Promise.reject(error);
        return {
          updateCallbackDone,
          ready: updateCallbackDone,
          finished: updateCallbackDone,
          skipTransition: () => {},
          types: new Set<string>(),
        };
      };

    await navigate("/update-failed");

    expect(log).toHaveBeenCalledWith("[zfb]", "Error", "update failed", error.stack);
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});

describe("history entry is committed outside the startViewTransition callback (zzmod#662)", () => {
  it("forward push: pushState runs BEFORE the startViewTransition callback", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("B", "page b"))),
    );

    const order: string[] = [];
    const origPush = History.prototype.pushState.bind(history);
    vi.spyOn(history, "pushState").mockImplementation(
      (...args: Parameters<History["pushState"]>) => {
        order.push("pushState");
        return origPush(...args);
      },
    );
    (document as unknown as { startViewTransition: unknown }).startViewTransition =
      makeFakeStartViewTransition(() => order.push("startViewTransition"));

    await navigate("/page-b");

    // Both must have happened, and the history write must precede the VT
    // callback — i.e. the entry is committed outside the callback frame, which
    // is exactly what WebKit requires to create a distinct back/forward entry.
    expect(order).toContain("pushState");
    expect(order).toContain("startViewTransition");
    expect(order.indexOf("pushState")).toBeLessThan(order.indexOf("startViewTransition"));
    // The swap still completed on the VT path.
    expect(document.querySelector("main")?.textContent).toBe("page b");
  });

  it("forward push: each nav adds one entry with a +1 index and commits the target URL", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) => htmlResponse(pageHtml("P", `at ${String(url)}`))),
    );

    const pushCalls: Array<{ index: number; url: string }> = [];
    const origPush = History.prototype.pushState.bind(history);
    vi.spyOn(history, "pushState").mockImplementation(
      (...args: Parameters<History["pushState"]>) => {
        const [state, , url] = args;
        pushCalls.push({ index: (state as { index: number }).index, url: String(url) });
        return origPush(...args);
      },
    );

    // Two forward navs. The tracked index is a module-level counter that
    // persists across tests, so assert the increment relatively (+1), not an
    // absolute value.
    await navigate("/page-b1");
    await navigate("/page-b2");

    expect(pushCalls).toHaveLength(2);
    expect(pushCalls[1]!.index).toBe(pushCalls[0]!.index + 1);
    expect(pushCalls[0]!.url).toContain("/page-b1");
    expect(pushCalls[1]!.url).toContain("/page-b2");
  });

  it("replace nav: uses replaceState (no new entry) before the callback, index preserved", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("R", "replaced"))),
    );

    const order: string[] = [];
    const pushSpy = vi
      .spyOn(history, "pushState")
      .mockImplementation((...args: Parameters<History["pushState"]>) => {
        order.push("pushState");
        return History.prototype.pushState.apply(history, args);
      });
    // The entry-committing replaceState carries a URL (3rd arg); the scroll-save
    // replaceState from updateScrollPosition does not — filter on the URL so we
    // only record the navigation's own write.
    const origReplace = History.prototype.replaceState.bind(history);
    vi.spyOn(history, "replaceState").mockImplementation(
      (...args: Parameters<History["replaceState"]>) => {
        const [state, , url] = args;
        if (url != null && String(url).includes("/replace-target")) {
          order.push(`replaceState:${(state as { index: number }).index}`);
        }
        return origReplace(...args);
      },
    );
    (document as unknown as { startViewTransition: unknown }).startViewTransition =
      makeFakeStartViewTransition(() => order.push("startViewTransition"));

    await navigate("/replace-target", { history: "replace" });

    // No new history entry was pushed.
    expect(pushSpy).not.toHaveBeenCalled();
    // The navigation's replaceState ran, preserved index 0, and came before the
    // VT callback.
    const replaceMarker = order.find((o) => o.startsWith("replaceState:"));
    expect(replaceMarker).toBe("replaceState:0");
    expect(order.indexOf(replaceMarker!)).toBeLessThan(order.indexOf("startViewTransition"));
  });

  it("back/forward (popstate) does NOT create a new history entry", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) => {
        const u = String(url);
        if (u.includes("/page-a")) return htmlResponse(pageHtml("A", "page a"));
        if (u.includes("/page-b")) return htmlResponse(pageHtml("B", "page b"));
        throw new Error(`unexpected fetch: ${u}`);
      }),
    );

    await navigate("/page-a");
    await navigate("/page-b");

    // Now drive a back navigation via popstate carrying history state.
    const pushSpy = vi.spyOn(history, "pushState");
    history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/page-a");
    window.dispatchEvent(
      new PopStateEvent("popstate", { state: { index: 0, scrollX: 0, scrollY: 0 } }),
    );
    await new Promise((r) => setTimeout(r, 0));

    // A traverse (historyState) navigation must not write a new entry — the
    // browser already moved.
    expect(pushSpy).not.toHaveBeenCalled();
  });
});

describe("zfb:before-swap `to` mutation does not double-push the history entry (#1398)", () => {
  it("a before-swap listener redirecting `to` produces exactly ONE new entry — the redirect corrects the URL via replaceState, not a second pushState", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => htmlResponse(pageHtml("B", "page b"))),
    );

    const pushCalls: Array<{ index: number; url: string }> = [];
    const origPush = History.prototype.pushState.bind(history);
    vi.spyOn(history, "pushState").mockImplementation(
      (...args: Parameters<History["pushState"]>) => {
        const [state, , url] = args;
        pushCalls.push({ index: (state as { index: number }).index, url: String(url) });
        return origPush(...args);
      },
    );

    const replaceCalls: Array<{ index: number; url: string }> = [];
    const origReplace = History.prototype.replaceState.bind(history);
    vi.spyOn(history, "replaceState").mockImplementation(
      (...args: Parameters<History["replaceState"]>) => {
        const [state, , url] = args;
        if (url != null) {
          replaceCalls.push({ index: (state as { index: number }).index, url: String(url) });
        }
        return origReplace(...args);
      },
    );

    // A zfb:before-swap listener redirects the destination — `to` is writable
    // on the event per Astro parity (events.ts BeforeEvent). By this point
    // transition()'s early WebKit-workaround commit has ALREADY pushed a new
    // entry for the ORIGINAL (pre-redirect) target.
    const redirectedTo = new URL("/page-b-redirected", location.href);
    const onBeforeSwap = (ev: Event) => {
      (ev as unknown as { to: URL }).to = redirectedTo;
    };
    document.addEventListener("zfb:before-swap", onBeforeSwap);

    await navigate("/page-b");

    document.removeEventListener("zfb:before-swap", onBeforeSwap);

    // Exactly one NEW entry for this navigation — the early commit. A second
    // pushState here would be the bug: one navigation, two entries, a phantom
    // Back stop.
    expect(pushCalls).toHaveLength(1);
    expect(pushCalls[0]!.url).toContain("/page-b");

    // The redirect still lands: moveToLocation corrects the already-committed
    // entry's URL to the listener-mutated `to` via replaceState — same index,
    // not a fresh one.
    const correctingReplace = replaceCalls.find((r) => r.url.includes("/page-b-redirected"));
    expect(correctingReplace).toBeDefined();
    expect(correctingReplace!.index).toBe(pushCalls[0]!.index);

    expect(location.pathname).toBe("/page-b-redirected");
  });
});

describe("Back wins after the early history commit but before swap (#2603)", () => {
  it("keeps page A and re-adopts its history index without teardown or post-swap lifecycle", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) =>
        htmlResponse(pageHtml("Page", `content for ${new URL(String(url)).pathname}`)),
      ),
    );

    // Establish the real outgoing page and then activate popstate handling at
    // that URL so originalLocation and currentHistoryIndex both describe A.
    await navigate("/race-page-a");
    init();
    const pageAIndex = (history.state as { index: number }).index;
    vi.mocked(cancelPendingIslands).mockClear();
    vi.mocked(unmountIslands).mockClear();
    vi.mocked(mountNewIslands).mockClear();

    const replaceCalls: string[] = [];
    const originalReplace = History.prototype.replaceState.bind(history);
    vi.spyOn(history, "replaceState").mockImplementation(
      (...args: Parameters<History["replaceState"]>) => {
        if (args[2] != null) replaceCalls.push(String(args[2]));
        return originalReplace(...args);
      },
    );
    const pushedIndexes: Array<{ index: number; url: string }> = [];
    const originalPush = History.prototype.pushState.bind(history);
    vi.spyOn(history, "pushState").mockImplementation(
      (...args: Parameters<History["pushState"]>) => {
        pushedIndexes.push({
          index: (args[0] as { index: number }).index,
          url: String(args[2]),
        });
        return originalPush(...args);
      },
    );

    const afterSwap = vi.fn();
    const pageLoad = vi.fn();
    const aborted = vi.fn();
    const onBeforeSwap = (event: Event) => {
      const swapEvent = event as Event & { to: URL };
      if (!swapEvent.to.pathname.endsWith("/race-page-b")) return;

      // Model the browser having already traversed Back to A while B's
      // old-page animation is yielding. onPopState aborts B synchronously and
      // the same-page fast path re-adopts A's finite index.
      history.replaceState({ index: pageAIndex, scrollX: 0, scrollY: 0 }, "", "/race-page-a");
      window.dispatchEvent(
        new PopStateEvent("popstate", {
          state: { index: pageAIndex, scrollX: 0, scrollY: 0 },
        }),
      );
    };
    document.addEventListener("zfb:before-swap", onBeforeSwap);
    document.addEventListener("zfb:after-swap", afterSwap);
    document.addEventListener("zfb:page-load", pageLoad);
    document.addEventListener("zfb:navigation-aborted", aborted);

    await navigate("/race-page-b");

    document.removeEventListener("zfb:before-swap", onBeforeSwap);
    document.removeEventListener("zfb:after-swap", afterSwap);
    document.removeEventListener("zfb:page-load", pageLoad);
    document.removeEventListener("zfb:navigation-aborted", aborted);

    expect(location.pathname).toBe("/race-page-a");
    expect(document.querySelector("main")?.textContent).toBe("content for /race-page-a");
    expect(replaceCalls.some((url) => url.includes("/race-page-b"))).toBe(false);
    expect(aborted).toHaveBeenCalledOnce();
    expect(afterSwap).not.toHaveBeenCalled();
    expect(pageLoad).not.toHaveBeenCalled();
    expect(cancelPendingIslands).not.toHaveBeenCalled();
    expect(unmountIslands).not.toHaveBeenCalled();
    expect(mountNewIslands).not.toHaveBeenCalled();
    expect(document.documentElement.hasAttribute("data-zfb-transition")).toBe(false);

    // A subsequent push must reuse A+1. If popstate failed to re-adopt A's
    // index, the aborted B commit would leave the counter one step too high.
    await navigate("/race-page-c");
    const pageCPush = pushedIndexes.find((entry) => entry.url.includes("/race-page-c"));
    expect(pageCPush?.index).toBe(pageAIndex + 1);
  });

  it("emits navigation-aborted when native VT skips the update callback entirely", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) =>
        htmlResponse(pageHtml("Page", `content for ${new URL(String(url)).pathname}`)),
      ),
    );

    await navigate("/skipped-callback-a");
    init();
    const pageAIndex = (history.state as { index: number }).index;
    vi.mocked(cancelPendingIslands).mockClear();
    vi.mocked(unmountIslands).mockClear();
    vi.mocked(mountNewIslands).mockClear();

    let settleUpdate!: () => void;
    const updateCallbackDone = new Promise<void>((resolve) => {
      settleUpdate = resolve;
    });
    const updateCallback = vi.fn();
    let markStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      markStarted = resolve;
    });
    (document as unknown as { startViewTransition: unknown }).startViewTransition = (
      callback: VTCallback,
    ): FakeViewTransition => {
      updateCallback.mockImplementation(callback);
      markStarted();
      return {
        updateCallbackDone,
        ready: updateCallbackDone,
        finished: updateCallbackDone,
        skipTransition: settleUpdate,
        types: new Set<string>(),
      };
    };

    const beforeSwap = vi.fn();
    const afterSwap = vi.fn();
    const pageLoad = vi.fn();
    const aborted = vi.fn();
    document.addEventListener("zfb:before-swap", beforeSwap);
    document.addEventListener("zfb:after-swap", afterSwap);
    document.addEventListener("zfb:page-load", pageLoad);
    document.addEventListener("zfb:navigation-aborted", aborted);

    const pageBNavigation = navigate("/skipped-callback-b");
    await started;
    expect(location.pathname).toBe("/skipped-callback-b");

    // Model a real Back traversal restoring A and Chromium canceling the
    // pending native transition without invoking its update callback.
    history.replaceState({ index: pageAIndex, scrollX: 0, scrollY: 0 }, "", "/skipped-callback-a");
    window.dispatchEvent(
      new PopStateEvent("popstate", {
        state: { index: pageAIndex, scrollX: 0, scrollY: 0 },
      }),
    );
    settleUpdate();
    await pageBNavigation;

    document.removeEventListener("zfb:before-swap", beforeSwap);
    document.removeEventListener("zfb:after-swap", afterSwap);
    document.removeEventListener("zfb:page-load", pageLoad);
    document.removeEventListener("zfb:navigation-aborted", aborted);

    expect(updateCallback).not.toHaveBeenCalled();
    expect(aborted).toHaveBeenCalledOnce();
    expect(beforeSwap).not.toHaveBeenCalled();
    expect(afterSwap).not.toHaveBeenCalled();
    expect(pageLoad).not.toHaveBeenCalled();
    expect(cancelPendingIslands).not.toHaveBeenCalled();
    expect(unmountIslands).not.toHaveBeenCalled();
    expect(mountNewIslands).not.toHaveBeenCalled();
    expect(location.pathname).toBe("/skipped-callback-a");
    expect(document.querySelector("main")?.textContent).toBe("content for /skipped-callback-a");
    expect(document.documentElement.hasAttribute("data-zfb-transition")).toBe(false);
  });
});

describe("rejected History writes on the native View Transition path (#4127)", () => {
  let loads: ReturnType<typeof captureDocumentLoads>;
  let events: string[];
  const LIFECYCLE = [
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ] as const;
  const record = (event: Event) => events.push(event.type);
  const abs = (path: string) => new URL(path, location.href).href;
  const rejection = () => new DOMException("History write rejected", "SecurityError");

  beforeEach(() => {
    loads = captureDocumentLoads();
    events = [];
    for (const type of LIFECYCLE) document.addEventListener(type, record);
    document.body.innerHTML = "<main>old page</main>";
    vi.mocked(cancelPendingIslands).mockClear();
    vi.mocked(unmountIslands).mockClear();
    vi.mocked(mountNewIslands).mockClear();
    vi.spyOn(console, "log").mockImplementation(() => {});
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: RequestInfo) =>
        htmlResponse(pageHtml("Page", `content for ${new URL(String(url)).pathname}`)),
      ),
    );
  });

  afterEach(() => {
    for (const type of LIFECYCLE) document.removeEventListener(type, record);
    loads.restore();
  });

  function expectOldPageKept(path: string): void {
    expect(location.pathname).toBe(path);
    expect(document.querySelector("main")?.textContent).toBe("old page");
    expect(events).not.toContain("zfb:after-swap");
    expect(events).not.toContain("zfb:page-load");
    expect(events.filter((type) => type === "zfb:navigation-aborted")).toHaveLength(1);
    expect(cancelPendingIslands).not.toHaveBeenCalled();
    expect(unmountIslands).not.toHaveBeenCalled();
    expect(mountNewIslands).not.toHaveBeenCalled();
    expect(document.documentElement.hasAttribute("data-zfb-transition")).toBe(false);
  }

  function retargetTo(path: string): () => void {
    const listener = (event: Event) => {
      (event as Event & { to: URL }).to = new URL(path, location.href);
    };
    document.addEventListener("zfb:before-swap", listener);
    return () => document.removeEventListener("zfb:before-swap", listener);
  }

  it("a rejected early push never starts the transition and recovers with one document load", async () => {
    const startViewTransition = vi.fn(makeFakeStartViewTransition());
    (document as unknown as { startViewTransition: unknown }).startViewTransition =
      startViewTransition;
    vi.spyOn(history, "pushState").mockImplementation(() => {
      throw rejection();
    });

    await navigate("/vt-rejected");

    expect(startViewTransition).not.toHaveBeenCalled();
    expectOldPageKept("/");
    expect(events).not.toContain("zfb:before-swap");
    expect(loads.assigned).toEqual([abs("/vt-rejected")]);
    expect(loads.replaced).toEqual([]);
  });

  it("early push, then a rejected correction inside the callback: replace-recovery before teardown", async () => {
    const pushes: string[] = [];
    const nativePush = History.prototype.pushState;
    vi.spyOn(history, "pushState").mockImplementation((...args) => {
      pushes.push(String(args[2]));
      return nativePush.apply(history, args);
    });
    const nativeReplace = History.prototype.replaceState;
    vi.spyOn(history, "replaceState").mockImplementation((...args) => {
      if (args[2] != null) throw rejection();
      return nativeReplace.apply(history, args);
    });
    const stop = retargetTo("/vt-final");

    await navigate("/vt-early");
    stop();

    expect(pushes).toEqual([abs("/vt-early")]);
    expectOldPageKept("/vt-early");
    expect(events.filter((type) => type === "zfb:before-swap")).toHaveLength(1);
    expect(loads.replaced).toEqual([abs("/vt-final")]);
    expect(loads.assigned).toEqual([]);
  });

  it("no early entry, late new target: no push inside the callback, document load to the final target", async () => {
    const pushState = vi.spyOn(history, "pushState");
    const stop = retargetTo("/vt-late");

    // The prepared destination is the live URL, so transition() writes nothing
    // before the callback; only the before-swap listener asks for a new entry.
    await navigate("/");
    stop();

    expect(pushState).not.toHaveBeenCalled();
    expectOldPageKept("/");
    expect(events.filter((type) => type === "zfb:before-swap")).toHaveLength(1);
    expect(loads.assigned).toEqual([abs("/vt-late")]);
    expect(loads.replaced).toEqual([]);
  });

  it("a submitted form swaps outside native View Transitions, writing History once after before-swap", async () => {
    const order: string[] = [];
    const startViewTransition = vi.fn(makeFakeStartViewTransition());
    (document as unknown as { startViewTransition: unknown }).startViewTransition =
      startViewTransition;
    const nativePush = History.prototype.pushState;
    vi.spyOn(history, "pushState").mockImplementation((...args) => {
      order.push(`pushState ${new URL(String(args[2])).pathname}`);
      return nativePush.apply(history, args);
    });
    const onBeforeSwap = () => order.push("before-swap");
    document.addEventListener("zfb:before-swap", onBeforeSwap);
    const formData = new FormData();
    formData.set("name", "value");

    await navigate("/vt-form", { formData });
    document.removeEventListener("zfb:before-swap", onBeforeSwap);

    expect(startViewTransition).not.toHaveBeenCalled();
    expect(order).toEqual(["before-swap", "pushState /vt-form"]);
    expect(location.pathname).toBe("/vt-form");
    expect(document.querySelector("main")?.textContent).toBe("content for /vt-form");
    expect(events.filter((type) => type === "zfb:after-swap")).toHaveLength(1);
  });
});
