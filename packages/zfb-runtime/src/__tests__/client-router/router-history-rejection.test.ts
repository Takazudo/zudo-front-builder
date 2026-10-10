/**
 * @vitest-environment happy-dom
 */
// Regression tests for #4127: a History URL write that the browser REJECTS in
// an ordinary document (WebKit's History-write rate limit, #4125) must not let
// the router complete an SPA navigation under the old URL.
//
// This file drives the simulated-transition path (happy-dom has no
// document.startViewTransition); router-vt-history.test.ts covers the native
// View Transition placement. Each rejection test is paired with the public
// consequences a user would see: the URL, the page content, the lifecycle
// events, island teardown, the full-document recovery (if any), and — through
// the NEXT successful navigation — the router's tracked index and "from" URL.

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

installHappyDomShim();

function enableTransitions(): void {
  const meta = document.createElement("meta");
  meta.setAttribute("name", "zfb-view-transitions-enabled");
  meta.setAttribute("content", "true");
  document.head.appendChild(meta);
}
enableTransitions();

if (typeof document.getAnimations !== "function") {
  Object.defineProperty(document, "getAnimations", { configurable: true, value: () => [] });
}
if (typeof (globalThis as { KeyframeEffect?: unknown }).KeyframeEffect === "undefined") {
  (globalThis as { KeyframeEffect: unknown }).KeyframeEffect = class KeyframeEffect {};
}

import {
  init,
  navigate,
  supportsViewTransitions,
  syncHistoryEntry,
} from "../../client-router/router.js";

init();

const LIFECYCLE = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
  "zfb:navigation-aborted",
] as const;

let loads: ReturnType<typeof captureDocumentLoads>;
let events: string[];
let froms: string[];
const recordEvent = (event: Event) => {
  events.push(event.type);
  if (event.type === "zfb:before-preparation") {
    froms.push((event as Event & { from: URL }).from.pathname);
  }
};

beforeEach(() => {
  resetDocument();
  enableTransitions();
  document.body.innerHTML = "<main>old page</main>";
  loads = captureDocumentLoads();
  events = [];
  froms = [];
  for (const type of LIFECYCLE) document.addEventListener(type, recordEvent);
  vi.mocked(cancelPendingIslands).mockClear();
  vi.mocked(unmountIslands).mockClear();
  vi.mocked(mountNewIslands).mockClear();
  vi.spyOn(console, "log").mockImplementation(() => {});
});

afterEach(async () => {
  for (const type of LIFECYCLE) document.removeEventListener(type, recordEvent);
  loads.restore();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  await drainHappyDom();
});

function htmlResponse(body: string): Response {
  return new Response(body, { headers: { "content-type": "text/html; charset=utf-8" } });
}
function pageHtml(content: string): string {
  return `<!doctype html><html><head>
    <meta name="zfb-view-transitions-enabled" content="true">
    <title>${content}</title>
  </head><body><main>${content}</main></body></html>`;
}
function stubFetchByPath(): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(async (url: RequestInfo) =>
    htmlResponse(pageHtml(`content for ${new URL(String(url)).pathname}`)),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

// WebKit reports its History-write limit as a SecurityError in an ordinary
// document — the same name a srcdoc restriction carries.
const historyRejection = () => new DOMException("History write rejected", "SecurityError");

type WriteMethod = "pushState" | "replaceState";

// One spy per History method for the whole test. URL-carrying writes are
// recorded as attempts; `reject(method, error)` makes the next one throw.
// Scroll-position saves (2-arg replaceState) pass through unless
// `rejectMetadataSaves` is set.
function controlHistory() {
  const pushes: Array<{ index: number; url: string }> = [];
  const attempts: Record<WriteMethod, string[]> = { pushState: [], replaceState: [] };
  const pending: Partial<Record<WriteMethod, unknown>> = {};
  const control = {
    pushes,
    attempts,
    rejectedMetadataSaves: 0,
    rejectMetadataSaves: false,
    reject(method: WriteMethod, error: unknown) {
      pending[method] = error;
    },
  };
  for (const method of ["pushState", "replaceState"] as const) {
    const native = History.prototype[method];
    vi.spyOn(history, method).mockImplementation((...args: Parameters<History["pushState"]>) => {
      if (args.length > 2 && args[2] != null) {
        attempts[method].push(String(args[2]));
        if (method in pending) {
          const error = pending[method];
          delete pending[method];
          throw error;
        }
      } else if (control.rejectMetadataSaves) {
        control.rejectedMetadataSaves++;
        throw historyRejection();
      }
      native.apply(history, args);
      if (method === "pushState") {
        pushes.push({ index: (args[0] as { index: number }).index, url: String(args[2]) });
      }
    });
  }
  return control;
}

const abs = (path: string) => new URL(path, location.href).href;

// Establish a real, router-managed current page and return its index.
async function startAt(path: string): Promise<number> {
  stubFetchByPath();
  await navigate(path);
  expect(location.pathname).toBe(path);
  events.length = 0;
  froms.length = 0;
  vi.mocked(cancelPendingIslands).mockClear();
  vi.mocked(unmountIslands).mockClear();
  vi.mocked(mountNewIslands).mockClear();
  return (history.state as { index: number }).index;
}

function expectNoSwap(page: string, path: string): void {
  expect(location.pathname).toBe(path);
  expect(document.querySelector("main")?.textContent).toBe(page);
  expect(events).not.toContain("zfb:after-swap");
  expect(events).not.toContain("zfb:page-load");
  expect(events.filter((type) => type === "zfb:navigation-aborted")).toHaveLength(1);
  expect(cancelPendingIslands).not.toHaveBeenCalled();
  expect(unmountIslands).not.toHaveBeenCalled();
  expect(mountNewIslands).not.toHaveBeenCalled();
  expect(document.documentElement.hasAttribute("data-zfb-transition")).toBe(false);
}

describe("precondition", () => {
  it("runs the simulated transition path", () => {
    expect(supportsViewTransitions).toBe(false);
  });
});

describe("ordinary GET push/replace rejection", () => {
  it("push: keeps the old page, recovers with one document load, and does not advance bookkeeping", async () => {
    const baseIndex = await startAt("/push-base");
    const h = controlHistory();
    h.reject("pushState", historyRejection());

    await navigate("/push-target");

    expectNoSwap("content for /push-base", "/push-base");
    expect(h.attempts.pushState).toEqual([abs("/push-target")]);
    expect(loads.assigned).toEqual([abs("/push-target")]);
    expect(loads.replaced).toEqual([]);

    // The next navigation starts from the page that is really current and
    // takes the very next index — the rejected push left no trace.
    events.length = 0;
    froms.length = 0;
    await navigate("/push-next");
    expect(froms).toEqual(["/push-base"]);
    expect(location.pathname).toBe("/push-next");
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/push-next") }]);
    expect(events).toContain("zfb:after-swap");
  });

  it("replace: recovers with location.replace and keeps the entry's index", async () => {
    const baseIndex = await startAt("/replace-base");
    const h = controlHistory();
    h.reject("replaceState", historyRejection());

    await navigate("/replace-target", { history: "replace" });

    expectNoSwap("content for /replace-base", "/replace-base");
    expect(h.attempts.replaceState).toEqual([abs("/replace-target")]);
    expect(loads.replaced).toEqual([abs("/replace-target")]);
    expect(loads.assigned).toEqual([]);
    expect((history.state as { index: number }).index).toBe(baseIndex);
  });

  it("recovers to the resolved (redirected) destination, keeping the fragment", async () => {
    await startAt("/redirect-base");
    const finalUrl = abs("/redirect-final");
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        const res = htmlResponse(pageHtml("redirected"));
        Object.defineProperty(res, "redirected", { value: true });
        Object.defineProperty(res, "url", { value: finalUrl });
        return res;
      }),
    );
    controlHistory().reject("pushState", historyRejection());

    await navigate("/redirect-old#part");

    expectNoSwap("content for /redirect-base", "/redirect-base");
    expect(loads.assigned).toEqual([`${finalUrl}#part`]);
  });

  it("success control: one push, one new index, the new page swapped in", async () => {
    const baseIndex = await startAt("/control-base");
    const h = controlHistory();

    await navigate("/control-next");

    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/control-next") }]);
    expect(document.querySelector("main")?.textContent).toBe("content for /control-next");
    expect(events).toContain("zfb:after-swap");
    expect(events).not.toContain("zfb:navigation-aborted");
    expect(loads.assigned).toEqual([]);
  });
});

describe("before-swap retargeting (final target commit before teardown)", () => {
  function retargetTo(path: string): () => void {
    const listener = (event: Event) => {
      (event as Event & { to: URL }).to = new URL(path, location.href);
    };
    document.addEventListener("zfb:before-swap", listener);
    return () => document.removeEventListener("zfb:before-swap", listener);
  }

  it("early push succeeded, rejected correction: no teardown, replace-recovery, no phantom entry", async () => {
    const baseIndex = await startAt("/retarget-base");
    const h = controlHistory();
    h.reject("replaceState", historyRejection());
    const stop = retargetTo("/retarget-final");

    await navigate("/retarget-early");
    stop();

    // The early entry exists (the browser accepted it); the correction was
    // rejected before any teardown, so the recovery REPLACES that entry
    // instead of adding another.
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/retarget-early") }]);
    expect(h.attempts.replaceState).toEqual([abs("/retarget-final")]);
    expectNoSwap("content for /retarget-base", "/retarget-early");
    expect(events.filter((type) => type === "zfb:before-swap")).toHaveLength(1);
    expect(loads.replaced).toEqual([abs("/retarget-final")]);
    expect(loads.assigned).toEqual([]);

    // Bookkeeping follows the browser's actual state: the early entry is the
    // current one, so the next navigation starts there and pushes after it.
    froms.length = 0;
    await navigate("/retarget-next");
    expect(froms).toEqual(["/retarget-early"]);
    expect(h.pushes.at(-1)).toEqual({ index: baseIndex + 2, url: abs("/retarget-next") });
  });

  it("no early entry (destination was the live URL), rejected late push: document load to the final target", async () => {
    await startAt("/same-url");
    const h = controlHistory();
    h.reject("pushState", historyRejection());
    const stop = retargetTo("/late-target");

    await navigate("/same-url");
    stop();

    expect(h.attempts.pushState).toEqual([abs("/late-target")]);
    expectNoSwap("content for /same-url", "/same-url");
    expect(events.filter((type) => type === "zfb:before-swap")).toHaveLength(1);
    expect(loads.assigned).toEqual([abs("/late-target")]);
    expect(loads.replaced).toEqual([]);
  });

  it("success control: a late new target on the simulated path pushes one entry", async () => {
    const baseIndex = await startAt("/same-url-ok");
    const h = controlHistory();
    const stop = retargetTo("/late-target-ok");

    await navigate("/same-url-ok");
    stop();

    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/late-target-ok") }]);
    expect(location.pathname).toBe("/late-target-ok");
    expect(document.querySelector("main")?.textContent).toBe("content for /same-url-ok");
    expect(events).toContain("zfb:after-swap");
  });
});

describe("hash/same-page fast path", () => {
  it("a rejected push aborts cleanly: no fragment, no scroll, no state advance", async () => {
    const baseIndex = await startAt("/hash-base");
    const fetchMock = stubFetchByPath();
    const h = controlHistory();
    h.reject("pushState", historyRejection());
    const scroll = vi.spyOn(window, "scrollTo");

    await navigate("/hash-base#section");

    expect(fetchMock).not.toHaveBeenCalled();
    expect(h.attempts.pushState).toEqual([abs("/hash-base#section")]);
    expect(location.pathname).toBe("/hash-base");
    expect(location.hash).toBe("");
    expect(loads.assigned).toEqual([]);
    expect(loads.replaced).toEqual([]);
    expect(scroll).not.toHaveBeenCalled();
    expect(events).toEqual(["zfb:navigation-aborted"]);

    // A later hash navigation works and takes the very next index.
    await navigate("/hash-base#other");
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/hash-base#other") }]);
    expect(location.hash).toBe("#other");
  });
});

describe("public syncHistoryEntry rejection", () => {
  it("push: rethrows the identical native error with no navigation side effects", async () => {
    const baseIndex = await startAt("/sync-base");
    const fetchMock = stubFetchByPath();
    const h = controlHistory();
    const error = historyRejection();
    h.reject("pushState", error);
    const scroll = vi.spyOn(window, "scrollTo");

    let thrown: unknown;
    try {
      syncHistoryEntry("/sync-dialog");
    } catch (caught) {
      thrown = caught;
    }

    expect(thrown).toBe(error);
    expect(location.pathname).toBe("/sync-base");
    expect(document.querySelector("main")?.textContent).toBe("content for /sync-base");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(scroll).not.toHaveBeenCalled();
    expect(events).toEqual([]);
    expect(loads.assigned).toEqual([]);
    expect(loads.replaced).toEqual([]);

    // Tracked state unchanged: the next push takes base+1 and the next
    // navigation starts from the page that is really current.
    syncHistoryEntry("/sync-dialog");
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/sync-dialog") }]);
    await navigate("/sync-next");
    expect(froms).toEqual(["/sync-dialog"]);
  });

  it("replace: rethrows the identical native error and leaves the entry and the from-URL alone", async () => {
    await startAt("/sync-replace-base");
    stubFetchByPath();
    const error = historyRejection();
    controlHistory().reject("replaceState", error);
    const stateBefore = history.state;

    let thrown: unknown;
    try {
      syncHistoryEntry("/sync-replace-dialog", { replace: true });
    } catch (caught) {
      thrown = caught;
    }

    expect(thrown).toBe(error);
    expect(history.state).toEqual(stateBefore);
    expect(location.pathname).toBe("/sync-replace-base");
    expect(events).toEqual([]);

    await navigate("/sync-replace-next");
    expect(froms).toEqual(["/sync-replace-base"]);
  });
});

describe("best-effort scroll saves", () => {
  it("a rejected scroll-position save does not block a valid navigation", async () => {
    const baseIndex = await startAt("/scroll-base");
    const h = controlHistory();
    h.rejectMetadataSaves = true;

    await navigate("/scroll-next");

    expect(h.rejectedMetadataSaves).toBeGreaterThan(0);
    expect(location.pathname).toBe("/scroll-next");
    expect(document.querySelector("main")?.textContent).toBe("content for /scroll-next");
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/scroll-next") }]);
    expect(events).toContain("zfb:after-swap");
  });
});

describe("submitted forms are never replayed", () => {
  function formData(): FormData {
    const data = new FormData();
    data.set("name", "value");
    return data;
  }
  function postCount(fetchMock: ReturnType<typeof vi.fn>): number {
    return fetchMock.mock.calls.filter(([, init]) => (init as RequestInit)?.method === "POST")
      .length;
  }

  it("rejection with no entry made: explicit abort, old page, POST sent once, no document load", async () => {
    const baseIndex = await startAt("/form-base");
    const fetchMock = stubFetchByPath();
    const h = controlHistory();
    h.reject("pushState", historyRejection());

    await navigate("/form-action", { formData: formData() });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(postCount(fetchMock)).toBe(1);
    expect(h.attempts.pushState).toEqual([abs("/form-action")]);
    expectNoSwap("content for /form-base", "/form-base");
    expect(loads.assigned).toEqual([]);
    expect(loads.replaced).toEqual([]);

    froms.length = 0;
    await navigate("/form-next");
    expect(froms).toEqual(["/form-base"]);
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/form-next") }]);
  });

  it("a redirected POST is not re-requested as a GET", async () => {
    await startAt("/form-redirect-base");
    const finalUrl = abs("/form-redirect-final");
    const fetchMock = vi.fn(async () => {
      const res = htmlResponse(pageHtml("posted"));
      Object.defineProperty(res, "redirected", { value: true });
      Object.defineProperty(res, "url", { value: finalUrl });
      return res;
    });
    vi.stubGlobal("fetch", fetchMock);
    controlHistory().reject("pushState", historyRejection());

    await navigate("/form-redirect-action", { formData: formData() });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expectNoSwap("content for /form-redirect-base", "/form-redirect-base");
    expect(loads.assigned).toEqual([]);
    expect(loads.replaced).toEqual([]);
  });

  // A form navigation writes History once, at the final target, right before
  // teardown — so neither an early push nor an early replace exists to strand
  // the old page under an intermediate URL when that write is rejected.
  it.each([
    ["push", undefined, "pushState"],
    ["replace", "replace", "replaceState"],
  ] as const)(
    "%s intent retargeted by before-swap: no intermediate write, old page under the old URL",
    async (_label, historyOption, method) => {
      const base = `/form-retarget-base-${method}`;
      await startAt(base);
      const fetchMock = stubFetchByPath();
      const h = controlHistory();
      h.reject(method, historyRejection());
      const listener = (event: Event) => {
        (event as Event & { to: URL }).to = new URL("/form-retarget-final", location.href);
      };
      document.addEventListener("zfb:before-swap", listener);

      await navigate("/form-retarget-action", {
        formData: formData(),
        ...(historyOption ? { history: historyOption } : {}),
      });
      document.removeEventListener("zfb:before-swap", listener);

      expect(postCount(fetchMock)).toBe(1);
      expect(fetchMock).toHaveBeenCalledTimes(1);
      // The only URL write is the single final one, which was rejected.
      expect(h.attempts).toEqual({
        pushState: method === "pushState" ? [abs("/form-retarget-final")] : [],
        replaceState: method === "replaceState" ? [abs("/form-retarget-final")] : [],
      });
      expectNoSwap(`content for ${base}`, base);
      expect(loads.assigned).toEqual([]);
      expect(loads.replaced).toEqual([]);
    },
  );

  it("success control: a form POST swaps in the response with one entry", async () => {
    const baseIndex = await startAt("/form-ok-base");
    const fetchMock = stubFetchByPath();
    const h = controlHistory();

    await navigate("/form-ok-action", { formData: formData() });

    expect(postCount(fetchMock)).toBe(1);
    expect(h.pushes).toEqual([{ index: baseIndex + 1, url: abs("/form-ok-action") }]);
    expect(location.pathname).toBe("/form-ok-action");
    expect(document.querySelector("main")?.textContent).toBe("content for /form-ok-action");
    expect(events.filter((type) => type === "zfb:after-swap")).toHaveLength(1);
  });
});
