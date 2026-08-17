/**
 * @vitest-environment happy-dom
 */
// Regression tests for #2424 — the client router must not throw when loaded
// inside an about:srcdoc document (e.g. an SPA-preview iframe shell), where
// Chromium refuses history.replaceState/pushState. router.ts's top-level
// init block calls history.replaceState synchronously at module-eval time
// once view transitions are enabled on the page and there is no existing
// history.state (L199-211) — a throwing native implementation must not
// propagate out of the module import itself.
//
// CRITICAL SEAM: the init block runs once, at module-eval time. The throwing
// history stub below must be installed BEFORE the router import that follows
// it in this file — vite-node (which powers Vitest) executes top-level
// statements and `import` declarations in textual order (see the "late
// import" comment in router-vt-history.test.ts), so positioning the stub
// ahead of the import is what makes it observe the throw.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

vi.mock("@takazudo/zfb/runtime", () => ({
  mountNewIslands: vi.fn(),
  cancelPendingIslands: vi.fn(),
  unmountIslands: vi.fn(),
}));

installHappyDomShim();

// Opt this page into view transitions BEFORE the router module evaluates —
// router.ts's init block reads the live document at module-eval time
// (transitionEnabledOnThisPage()) to decide whether to seed history.
function enableTransitions(): void {
  const meta = document.createElement("meta");
  meta.setAttribute("name", "zfb-view-transitions-enabled");
  meta.setAttribute("content", "true");
  document.head.appendChild(meta);
}
enableTransitions();

// Simulate the about:srcdoc restriction: Chromium throws when
// history.replaceState/pushState is called inside a srcdoc document. Stub
// both methods to throw before the late router import below, so the
// module's top-level init block — which calls history.replaceState
// unconditionally once transitions are enabled and history.state is empty —
// exercises the guard at the exact point Chromium would throw.
history.replaceState = () => {
  throw new DOMException("Failed to execute 'replaceState' on 'History'", "SecurityError");
};
history.pushState = () => {
  throw new DOMException("Failed to execute 'pushState' on 'History'", "SecurityError");
};

// Late import — router.ts's top-level init block runs here, against the
// throwing history stubs installed above. If safeReplaceState did not
// swallow the throw, importing this module would itself throw and this
// whole test file would fail to load (no test below would ever run).
import { syncHistoryEntry } from "../../client-router/router.js";

beforeEach(() => {
  resetDocument();
  enableTransitions();
});

afterEach(async () => {
  vi.unstubAllGlobals();
  await drainHappyDom();
});

describe("about:srcdoc tolerance (#2424)", () => {
  it("module init does not throw when history.replaceState throws", () => {
    // The late import above already exercised the throwing path at
    // module-eval time. Reaching this assertion at all is the proof — a
    // propagated throw would have failed this whole file to load, not just
    // this one test.
    expect(true).toBe(true);
  });

  it("syncHistoryEntry() (push) degrades silently when history.pushState throws", () => {
    expect(() => syncHistoryEntry("/detail")).not.toThrow();
  });

  it("syncHistoryEntry({ replace: true }) degrades silently when history.replaceState throws", () => {
    expect(() => syncHistoryEntry("/detail", { replace: true })).not.toThrow();
  });
});
