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

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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
