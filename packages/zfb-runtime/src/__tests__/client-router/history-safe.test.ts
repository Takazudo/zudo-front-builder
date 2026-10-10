/**
 * @vitest-environment happy-dom
 */
// Unit tests for client-router/history-safe — the History write wrappers used
// by every history.replaceState/pushState call site in router.ts and events.ts
// (#2424, #4127).
//
// Coverage:
//   - committed writes: passthrough behavior is unchanged (same call, same
//     arguments) and reported as committed.
//   - arity is preserved: a 2-arg call reaches the native method as a 2-arg
//     call, not a 3-arg call with an explicit `undefined` url.
//   - a SecurityError is tolerated only when the document itself is a srcdoc
//     document; the same error in an ordinary document is a rejection that
//     carries the browser's own error object.
//   - safeReplaceState (metadata writes) stays best-effort for any failure.

import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import {
  safeReplaceState,
  tryPushState,
  tryReplaceState,
} from "../../client-router/history-safe.js";

beforeEach(resetDocument);
afterEach(async () => {
  vi.restoreAllMocks();
  await drainHappyDom();
});

// happy-dom cannot load a real srcdoc iframe document, so this seam makes the
// live document report the address a srcdoc document has. Its baseURI stays
// the inherited HTTP URL, as in a real srcdoc iframe.
function enterSrcdocDocument(): void {
  vi.spyOn(document, "URL", "get").mockReturnValue("about:srcdoc");
}

function rejectWrites(error: unknown): void {
  vi.spyOn(history, "replaceState").mockImplementation(() => {
    throw error;
  });
  vi.spyOn(history, "pushState").mockImplementation(() => {
    throw error;
  });
}

describe("tryReplaceState — committed write (passthrough)", () => {
  it("writes history.state exactly like the native call and reports committed", () => {
    expect(tryReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "")).toEqual({
      outcome: "committed",
    });
    expect(history.state).toMatchObject({ index: 1, scrollX: 0, scrollY: 0 });
  });

  it("forwards a 2-arg call to the native method with the same arity (no explicit undefined url)", () => {
    const spy = vi.spyOn(history, "replaceState");
    tryReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "");
    expect(spy.mock.calls[0]).toHaveLength(2);
  });

  it("forwards the url argument when given", () => {
    const spy = vi.spyOn(history, "replaceState");
    tryReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "", "/detail");
    expect(spy.mock.calls[0]).toEqual([{ index: 1, scrollX: 0, scrollY: 0 }, "", "/detail"]);
  });
});

describe("tryPushState — committed write (passthrough)", () => {
  it("writes history.state exactly like the native call and reports committed", () => {
    expect(tryPushState({ index: 2, scrollX: 0, scrollY: 0 }, "", "/next")).toEqual({
      outcome: "committed",
    });
    expect(history.state).toMatchObject({ index: 2, scrollX: 0, scrollY: 0 });
    expect(location.pathname).toBe("/next");
  });

  it("forwards the exact arguments", () => {
    const spy = vi.spyOn(history, "pushState");
    tryPushState({ index: 3, scrollX: 0, scrollY: 0 }, "", "/again");
    expect(spy.mock.calls[0]).toEqual([{ index: 3, scrollX: 0, scrollY: 0 }, "", "/again"]);
  });
});

describe("rejected writes in an ordinary document (#4127)", () => {
  it("reports a SecurityError as rejected, carrying the identical error object", () => {
    expect(document.URL).toMatch(/^http/);
    const error = new DOMException("History write rejected", "SecurityError");
    rejectWrites(error);
    const push = tryPushState({ index: 1, scrollX: 0, scrollY: 0 }, "", "/next");
    const replace = tryReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "", "/next");
    expect(push.outcome).toBe("rejected");
    expect(replace.outcome).toBe("rejected");
    expect(push.outcome === "rejected" && push.error).toBe(error);
    expect(replace.outcome === "rejected" && replace.error).toBe(error);
  });

  it("reports any other thrown error as rejected", () => {
    const error = new TypeError("boom");
    rejectWrites(error);
    expect(tryPushState({}, "", "/next")).toEqual({ outcome: "rejected", error });
  });
});

describe("about:srcdoc tolerance (#2424)", () => {
  beforeEach(enterSrcdocDocument);

  it("tolerates a SecurityError when the document is a srcdoc document", () => {
    expect(document.baseURI).toMatch(/^http/);
    const error = new DOMException("replaceState is not allowed", "SecurityError");
    rejectWrites(error);
    expect(tryReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "")).toEqual({
      outcome: "tolerated",
      error,
    });
    expect(tryPushState({ index: 1, scrollX: 0, scrollY: 0 }, "", "/next")).toEqual({
      outcome: "tolerated",
      error,
    });
  });

  it("still tolerates the SecurityError after a fragment navigation inside the srcdoc document", () => {
    vi.spyOn(document, "URL", "get").mockReturnValue("about:srcdoc#section");
    const error = new DOMException("pushState is not allowed", "SecurityError");
    rejectWrites(error);
    expect(tryPushState({}, "", "/next")).toEqual({ outcome: "tolerated", error });
  });

  it("does not tolerate an unrelated error just because the document is srcdoc", () => {
    const error = new TypeError("boom");
    rejectWrites(error);
    expect(tryPushState({}, "", "/next")).toEqual({ outcome: "rejected", error });
  });
});

describe("safeReplaceState — best-effort metadata writes", () => {
  it("writes history.state exactly like the native call", () => {
    safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "");
    expect(history.state).toMatchObject({ index: 1, scrollX: 0, scrollY: 0 });
  });

  it("forwards a 2-arg call to the native method with the same arity", () => {
    const spy = vi.spyOn(history, "replaceState");
    safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "");
    expect(spy.mock.calls[0]).toHaveLength(2);
  });

  it("does not throw when an ordinary document rejects the write", () => {
    rejectWrites(new DOMException("History write rejected", "SecurityError"));
    expect(() => safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "")).not.toThrow();
  });

  it("does not throw inside a srcdoc document", () => {
    enterSrcdocDocument();
    rejectWrites(new DOMException("replaceState is not allowed", "SecurityError"));
    expect(() => safeReplaceState({ index: 1, scrollX: 0, scrollY: 0 }, "")).not.toThrow();
  });
});
