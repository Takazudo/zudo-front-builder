// Pin the reload wire contract between the Rust emitter (livereload.rs) and the
// browser consumer (crates/zfb-server/src/livereload.js).
//
// The critical invariant: when the islands bundle changes via a runtime-only
// diff, the Rust side emits component="" (empty string) because it doesn't know
// which components were affected. The JS consumer must NOT short-circuit on an
// empty component — it reads only `bundleUrl` to build the swap URL and must
// still trigger the full bundle re-import.
//
// These tests exercise the actual livereload.js source so that a future refactor
// which starts rejecting component=="" will fail here immediately.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

const here = dirname(fileURLToPath(import.meta.url));
// Cross-package path: from packages/zfb/src/__tests__/ up to the worktree root,
// then into the Rust crate that owns the browser-side reload client.
const LIVERELOAD_JS = resolve(here, "../../../../crates/zfb-server/src/livereload.js");

type EventHandler = (ev: { data: string }) => void;

interface FakeSourceInstance {
  readonly url: string;
  readonly listenerNames: string[];
  closeCalls: number;
  close(): void;
  dispatch(event: string, data: string): void;
}

interface LivereloadHarness {
  readonly instances: FakeSourceInstance[];
  cleanup(): void;
}

function setupLivereloadScript(scriptSrc?: string): LivereloadHarness {
  if (scriptSrc) {
    vi.spyOn(document, "currentScript", "get").mockReturnValue(
      Object.assign(document.createElement("script"), { src: scriptSrc }),
    );
  }
  vi.stubGlobal("location", new URL("http://localhost:4321/"));
  const instances: FakeSourceInstance[] = [];
  const lifecycleListeners: Array<{
    type: string;
    listener: EventListenerOrEventListenerObject;
    options?: boolean | AddEventListenerOptions;
  }> = [];
  const originalAddEventListener = window.addEventListener;
  const originalRemoveEventListener = window.removeEventListener;

  function FakeWebSocket(url: string) {
    const listeners: Record<string, EventHandler[]> = {};
    const instance: FakeSourceInstance & {
      addEventListener(name: string, handler: EventHandler): void;
    } = {
      url,
      listenerNames: [],
      closeCalls: 0,
      addEventListener(name: string, handler: EventHandler) {
        listeners[name] ??= [];
        listeners[name].push(handler);
        instance.listenerNames.push(name);
      },
      close() {
        instance.closeCalls += 1;
      },
      dispatch(event: string, data: string) {
        for (const fn of listeners[
          ["page", "css", "islands"].includes(event) ? "message" : event
        ] ?? []) {
          fn({
            data: ["page", "css", "islands"].includes(event)
              ? JSON.stringify({ event, data })
              : data,
          });
        }
      },
    };
    instances.push(instance);
    return instance;
  }

  vi.stubGlobal("WebSocket", FakeWebSocket);

  window.addEventListener = function (
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: boolean | AddEventListenerOptions,
  ) {
    if (type === "pagehide" || type === "pageshow") {
      lifecycleListeners.push({ type, listener, options });
    }
    originalAddEventListener.call(window, type, listener, options);
  } as typeof window.addEventListener;

  // Execute livereload.js in the global scope via new Function so the IIFE
  // sees window (set by happy-dom) and our stubbed WebSocket.
  const code = readFileSync(LIVERELOAD_JS, "utf-8");
  try {
    new Function(code)();
  } finally {
    window.addEventListener = originalAddEventListener;
  }

  return {
    instances,
    cleanup() {
      for (const { type, listener, options } of lifecycleListeners) {
        originalRemoveEventListener.call(window, type, listener, options);
      }
    },
  };
}

function pageTransitionEvent(type: "pagehide" | "pageshow", persisted: boolean): Event {
  const event = new Event(type);
  Object.defineProperty(event, "persisted", { value: persisted });
  return event;
}

describe("livereload.js WebSocket consumer — islands wire contract", () => {
  let harness: LivereloadHarness;
  let src: FakeSourceInstance;

  beforeEach(() => {
    harness = setupLivereloadScript();
    src = harness.instances[0];
  });

  afterEach(() => {
    window.dispatchEvent(pageTransitionEvent("pagehide", false));
    harness.cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    delete (window as unknown as Record<string, unknown>)["__zfbIslandsReload"];
  });

  it("empty component triggers bundle re-import keyed by bundleUrl (core contract)", () => {
    // This is the critical regression guard. The Rust emitter sends component=""
    // when only a runtime-only file changed (no specific component known). The
    // consumer must still fire __zfbIslandsReload / dynamic-import using bundleUrl.
    // If a future refactor short-circuits on component=="" this test must FAIL.
    const hook = vi.fn();
    (window as unknown as Record<string, unknown>)["__zfbIslandsReload"] = hook;

    src.dispatch("islands", JSON.stringify({ bundleUrl: "/assets/islands-abc.js", component: "" }));

    expect(hook).toHaveBeenCalledTimes(1);
    const [component, swapUrl] = hook.mock.calls[0] as [string, string];
    // Consumer passes component through unchanged — empty string is fine
    expect(component).toBe("");
    // swapUrl is bundleUrl with a ?v=<timestamp> cache-buster appended
    expect(swapUrl).toMatch(/^\/assets\/islands-abc\.js\?v=\d+$/);
  });

  it("named component also triggers the hook (normal hot-swap path)", () => {
    const hook = vi.fn();
    (window as unknown as Record<string, unknown>)["__zfbIslandsReload"] = hook;

    src.dispatch(
      "islands",
      JSON.stringify({ bundleUrl: "/assets/islands-abc.js", component: "Counter" }),
    );

    expect(hook).toHaveBeenCalledTimes(1);
    const [component, swapUrl] = hook.mock.calls[0] as [string, string];
    expect(component).toBe("Counter");
    expect(swapUrl).toMatch(/^\/assets\/islands-abc\.js\?v=\d+$/);
  });

  it("missing bundleUrl is silently ignored (no crash, no hook call)", () => {
    const hook = vi.fn();
    (window as unknown as Record<string, unknown>)["__zfbIslandsReload"] = hook;

    // Only component, no bundleUrl — the handler bails early
    src.dispatch("islands", JSON.stringify({ component: "Counter" }));

    expect(hook).not.toHaveBeenCalled();
  });

  it("malformed JSON payload is silently ignored (no crash)", () => {
    const hook = vi.fn();
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    (window as unknown as Record<string, unknown>)["__zfbIslandsReload"] = hook;

    expect(() => src.dispatch("islands", "not-json{{{")).not.toThrow();
    expect(hook).not.toHaveBeenCalled();

    warnSpy.mockRestore();
  });
});

describe("livereload.js page lifecycle", () => {
  let harness: LivereloadHarness;

  beforeEach(() => {
    harness = setupLivereloadScript();
  });

  afterEach(() => {
    window.dispatchEvent(pageTransitionEvent("pagehide", false));
    harness.cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("closes the current source on pagehide", () => {
    window.dispatchEvent(pageTransitionEvent("pagehide", false));

    expect(harness.instances).toHaveLength(1);
    expect(harness.instances[0].closeCalls).toBe(1);
  });

  it("reconnects a disconnected bfcache-restored page with all listeners", () => {
    window.dispatchEvent(pageTransitionEvent("pagehide", true));
    window.dispatchEvent(pageTransitionEvent("pageshow", true));

    expect(harness.instances).toHaveLength(2);
    expect(harness.instances[0].closeCalls).toBe(1);
    expect(harness.instances[1].url).toBe(harness.instances[0].url);
    expect(harness.instances[1].listenerNames).toEqual(["open", "message", "close", "error"]);
  });

  it("does not reconnect on non-persisted pageshow after pagehide", () => {
    window.dispatchEvent(pageTransitionEvent("pagehide", false));
    window.dispatchEvent(pageTransitionEvent("pageshow", false));

    expect(harness.instances).toHaveLength(1);
  });

  it("does not double-connect on persisted pageshow while connected", () => {
    window.dispatchEvent(pageTransitionEvent("pageshow", true));

    expect(harness.instances).toHaveLength(1);
    expect(harness.instances[0].closeCalls).toBe(0);
  });

  it("retries a dropped connection with backoff and cancels retries on pagehide", () => {
    vi.useFakeTimers();
    harness.instances[0].dispatch("close", "");
    vi.advanceTimersByTime(1000);
    expect(harness.instances).toHaveLength(2);
    harness.instances[1].dispatch("close", "");
    vi.advanceTimersByTime(1000);
    expect(harness.instances).toHaveLength(2);
    vi.advanceTimersByTime(1000);
    expect(harness.instances).toHaveLength(3);
    harness.instances[2].dispatch("close", "");
    window.dispatchEvent(pageTransitionEvent("pagehide", true));
    vi.advanceTimersByTime(20000);
    expect(harness.instances).toHaveLength(3);
    window.dispatchEvent(pageTransitionEvent("pageshow", true));
    expect(harness.instances).toHaveLength(4);
  });

  it("ignores late close and message events from a previous document connection", () => {
    const hook = vi.fn();
    (window as unknown as Record<string, unknown>)["__zfbIslandsReload"] = hook;
    window.dispatchEvent(pageTransitionEvent("pagehide", true));
    window.dispatchEvent(pageTransitionEvent("pageshow", true));
    harness.instances[0].dispatch("islands", JSON.stringify({ bundleUrl: "/old.js" }));
    harness.instances[0].dispatch("close", "");
    expect(hook).not.toHaveBeenCalled();
    expect(harness.instances).toHaveLength(2);
    delete (window as unknown as Record<string, unknown>)["__zfbIslandsReload"];
  });

  it("keeps the source open on visibilitychange", () => {
    document.dispatchEvent(new Event("visibilitychange"));

    expect(harness.instances).toHaveLength(1);
    expect(harness.instances[0].closeCalls).toBe(0);
  });
});

describe("livereload.js socket URL", () => {
  it.each([
    [
      "http://localhost:4321/foo/__zfb/livereload.js?v=1",
      "ws://localhost:4321/foo/__zfb/reload/ws",
    ],
    ["https://example.test/foo/__zfb/livereload.js", "wss://example.test/foo/__zfb/reload/ws"],
  ])("derives a mount-aware socket from %s", (scriptSrc, expected) => {
    const harness = setupLivereloadScript(scriptSrc);
    try {
      expect(harness.instances[0].url).toBe(expected);
    } finally {
      window.dispatchEvent(pageTransitionEvent("pagehide", false));
      harness.cleanup();
      vi.unstubAllGlobals();
      vi.restoreAllMocks();
    }
  });
});

describe("livereload.js css reload URLs", () => {
  let harness: LivereloadHarness;
  let src: FakeSourceInstance;
  const T1 = 1_700_000_000_000;
  const T2 = 1_700_000_000_500;

  beforeEach(() => {
    document.head.innerHTML = "";
    harness = setupLivereloadScript();
    src = harness.instances[0];
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(T1);
  });

  afterEach(() => {
    window.dispatchEvent(pageTransitionEvent("pagehide", false));
    harness.cleanup();
    document.head.innerHTML = "";
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function addStylesheet(href: string): HTMLLinkElement {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.setAttribute("href", href);
    document.head.append(link);
    return link;
  }

  function setBase(href: string): void {
    const base = document.createElement("base");
    base.href = href;
    document.head.prepend(base);
  }

  it.each([
    [
      "functional query and fragment",
      "/theme.css?theme=dark&v=old#sheet",
      `https://site.test/theme.css?theme=dark&v=${T1}#sheet`,
    ],
    ["fragment only", "/base/style.css#theme", `https://site.test/base/style.css?v=${T1}#theme`],
    [
      "repeated non-v parameters",
      "/a.css?tag=x&tag=y&v=1&v=2",
      `https://site.test/a.css?tag=x&tag=y&v=${T1}`,
    ],
    [
      "protocol-relative font stylesheet",
      "//fonts.example.test/css2?family=Inter:wght@400&display=swap",
      `https://fonts.example.test/css2?family=Inter%3Awght%40400&display=swap&v=${T1}`,
    ],
    ["plain root-relative", "/style.css", `https://site.test/style.css?v=${T1}`],
    [
      "document-relative under <base>",
      "theme.css?x=1",
      `https://site.test/sub/docs/theme.css?x=1&v=${T1}`,
    ],
    [
      "parent-relative under <base>",
      "../shared.css#f",
      `https://site.test/sub/shared.css?v=${T1}#f`,
    ],
  ])("%s keeps its meaning and gets a request-query v", (_label, href, expected) => {
    setBase("https://site.test/sub/docs/page.html");
    const link = addStylesheet(href);

    src.dispatch("css", "");

    expect(link.getAttribute("href")).toBe(expected);
    const url = new URL(link.getAttribute("href")!);
    expect(url.searchParams.getAll("v")).toEqual([String(T1)]);
  });

  it("resolves a relative href against the document URL without <base>", () => {
    const link = addStylesheet("styles/site.css?mode=a");

    src.dispatch("css", "");

    expect(link.getAttribute("href")).toBe(
      new URL(`styles/site.css?mode=a&v=${T1}`, document.baseURI).href,
    );
  });

  it("repeated reloads keep a single, updated v", () => {
    setBase("https://site.test/");
    const link = addStylesheet("/theme.css?theme=dark#sheet");

    src.dispatch("css", "");
    vi.setSystemTime(T2);
    src.dispatch("css", "");

    expect(link.getAttribute("href")).toBe(`https://site.test/theme.css?theme=dark&v=${T2}#sheet`);
  });

  it("a malformed href does not stop later valid stylesheets from reloading", () => {
    setBase("https://site.test/");
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const bad = addStylesheet("http://[malformed/style.css");
    const good = addStylesheet("/after.css?keep=1");

    expect(() => src.dispatch("css", "")).not.toThrow();

    expect(bad.getAttribute("href")).toBe("http://[malformed/style.css");
    expect(good.getAttribute("href")).toBe(`https://site.test/after.css?keep=1&v=${T1}`);
    expect(warnSpy).toHaveBeenCalled();
  });
});
