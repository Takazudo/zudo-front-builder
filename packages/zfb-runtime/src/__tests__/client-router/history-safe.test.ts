/**
 * @vitest-environment happy-dom
 */
// Unit tests for client-router/history-safe — the guard used by every
// history.replaceState/pushState call site in router.ts and events.ts (#2424).
//
// Coverage:
//   - normal document: passthrough behavior is unchanged (same call, same
//     arguments, no swallowing) — the try/catch must be a no-op here.
//   - throwing document (simulating about:srcdoc): the throw is swallowed
//     rather than propagated.
//   - arity is preserved: a 2-arg call reaches the native method as a 2-arg
//     call, not a 3-arg call with an explicit `undefined` url.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import { safePushState, safeReplaceState } from "../../client-router/history-safe.js";

beforeEach(resetDocument);
afterEach(async () => {
  vi.restoreAllMocks();
  await drainHappyDom();
});

describe("safeReplaceState — normal document (passthrough)", () => {
  it("writes history.state exactly like the native call", () => {
    safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "");
    expect(history.state).toMatchObject({ index: 1, scrollX: 0, scrollY: 0 });
  });

  it("forwards a 2-arg call to the native method with the same arity (no explicit undefined url)", () => {
    const spy = vi.spyOn(history, "replaceState");
    safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "");
    expect(spy.mock.calls[0]).toHaveLength(2);
  });

  it("forwards the url argument when given", () => {
    const spy = vi.spyOn(history, "replaceState");
    safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "", "/detail");
    expect(spy.mock.calls[0]).toEqual([{ index: 1, scrollX: 0, scrollY: 0 }, "", "/detail"]);
  });
});

describe("safePushState — normal document (passthrough)", () => {
  it("writes history.state exactly like the native call", () => {
    safePushState({ index: 2, scrollX: 0, scrollY: 0 }, "", "/next");
    expect(history.state).toMatchObject({ index: 2, scrollX: 0, scrollY: 0 });
  });
});

describe("about:srcdoc tolerance — throwing history (#2424)", () => {
  // Chromium refuses history.replaceState/pushState inside an about:srcdoc
  // document (e.g. an SPA-preview iframe shell) and throws. Simulate that by
  // stubbing the native methods to throw, and assert the guard swallows it.
  it("safeReplaceState does not throw when history.replaceState throws", () => {
    vi.spyOn(history, "replaceState").mockImplementation(() => {
      throw new DOMException("replaceState is not allowed", "SecurityError");
    });
    expect(() => safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "")).not.toThrow();
  });

  it("safePushState does not throw when history.pushState throws", () => {
    vi.spyOn(history, "pushState").mockImplementation(() => {
      throw new DOMException("pushState is not allowed", "SecurityError");
    });
    expect(() => safePushState({ index: 1, scrollX: 0, scrollY: 0 }, "", "/next")).not.toThrow();
  });
});
