// Pin the SSE wire contract between the Rust emitter (livereload.rs) and the
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
// then into the Rust crate that owns the browser-side SSE client.
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

function setupLivereloadScript(): LivereloadHarness {
  const instances: FakeSourceInstance[] = [];
  const lifecycleListeners: Array<{
    type: string;
    listener: EventListenerOrEventListenerObject;
    options?: boolean | AddEventListenerOptions;
  }> = [];
  const originalAddEventListener = window.addEventListener;
  const originalRemoveEventListener = window.removeEventListener;

  function FakeEventSource(url: string) {
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
        for (const fn of listeners[event] ?? []) {
          fn({ data });
        }
      },
    };
    instances.push(instance);
    return instance;
  }

  vi.stubGlobal("EventSource", FakeEventSource);

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
  // sees window (set by happy-dom) and our stubbed EventSource.
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

describe("livereload.js SSE consumer — islands wire contract", () => {
  let harness: LivereloadHarness;
  let src: FakeSourceInstance;

  beforeEach(() => {
    harness = setupLivereloadScript();
    src = harness.instances[0];
  });

  afterEach(() => {
    harness.cleanup();
    vi.unstubAllGlobals();
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
    harness.cleanup();
    vi.unstubAllGlobals();
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
    expect(harness.instances[1].listenerNames).toEqual(["page", "css", "islands", "error"]);
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

  it("keeps the source open on visibilitychange", () => {
    document.dispatchEvent(new Event("visibilitychange"));

    expect(harness.instances).toHaveLength(1);
    expect(harness.instances[0].closeCalls).toBe(0);
  });
});
