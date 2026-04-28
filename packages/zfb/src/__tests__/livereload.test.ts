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
  dispatch(event: string, data: string): void;
}

function setupLivereloadScript(): FakeSourceInstance {
  const listeners: Record<string, EventHandler[]> = {};

  function FakeEventSource(_url: string) {
    return {
      addEventListener(name: string, handler: EventHandler) {
        listeners[name] ??= [];
        listeners[name].push(handler);
      },
    };
  }

  vi.stubGlobal("EventSource", FakeEventSource);

  // Execute livereload.js in the global scope via new Function so the IIFE
  // sees window (set by happy-dom) and our stubbed EventSource.
  const code = readFileSync(LIVERELOAD_JS, "utf-8");
  new Function(code)();

  return {
    dispatch(event: string, data: string) {
      for (const fn of listeners[event] ?? []) {
        fn({ data });
      }
    },
  };
}

describe("livereload.js SSE consumer — islands wire contract", () => {
  let src: FakeSourceInstance;

  beforeEach(() => {
    src = setupLivereloadScript();
  });

  afterEach(() => {
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
