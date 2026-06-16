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

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

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
import { navigate, supportsViewTransitions } from "../../client-router/router.js";

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
