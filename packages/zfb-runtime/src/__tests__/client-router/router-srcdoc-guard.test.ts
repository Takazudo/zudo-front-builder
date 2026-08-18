/**
 * @vitest-environment happy-dom
 */
// Regression tests for #2424 — the client router must not throw when loaded
// inside an about:srcdoc document (e.g. an SPA-preview iframe shell), where
// Chromium refuses history.replaceState/pushState.
//
// CRITICAL SEAM (rewritten for #2436): the history seed used to run at
// module-eval time, so the original version of this file installed the
// throwing history stub before a deliberately late router import and treated
// "the import did not throw" as the proof. Since #2436 the seed runs inside
// init() instead, so the stub only has to be in place before the init() call
// — and the assertion can be a direct `expect(init).not.toThrow()` rather than
// an implicit whole-file-loads check.
//
// The seed reaches history.replaceState only on the branch where the page is
// opted into view transitions AND has no existing history.state, so both
// preconditions are set up explicitly before each exercise.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

vi.mock("@takazudo/zfb/runtime", () => ({
  mountNewIslands: vi.fn(),
  cancelPendingIslands: vi.fn(),
  unmountIslands: vi.fn(),
}));

installHappyDomShim();

import { init, syncHistoryEntry } from "../../client-router/router.js";

// Simulate the about:srcdoc restriction: Chromium throws when
// history.replaceState/pushState is called inside a srcdoc document.
function installThrowingHistory(): void {
  history.replaceState = () => {
    throw new DOMException("Failed to execute 'replaceState' on 'History'", "SecurityError");
  };
  history.pushState = () => {
    throw new DOMException("Failed to execute 'pushState' on 'History'", "SecurityError");
  };
}

// Opt the page into view transitions — the seed only calls replaceState on the
// `transitionEnabledOnThisPage()` branch.
function enableTransitions(): void {
  const meta = document.createElement("meta");
  meta.setAttribute("name", "zfb-view-transitions-enabled");
  meta.setAttribute("content", "true");
  document.head.appendChild(meta);
}

beforeEach(() => {
  resetDocument();
  enableTransitions();
  installThrowingHistory();
});

afterEach(async () => {
  vi.unstubAllGlobals();
  await drainHappyDom();
});

describe("about:srcdoc tolerance (#2424)", () => {
  it("init() does not throw when history.replaceState throws", () => {
    // history.state is null in a fresh happy-dom document and the throwing
    // stub keeps it that way, so init()'s seed takes the replaceState branch —
    // the exact point Chromium throws inside a srcdoc document.
    expect(history.state).toBeNull();
    expect(() => init()).not.toThrow();
  });

  it("syncHistoryEntry() (push) degrades silently when history.pushState throws", () => {
    expect(() => syncHistoryEntry("/detail")).not.toThrow();
  });

  it("syncHistoryEntry({ replace: true }) degrades silently when history.replaceState throws", () => {
    expect(() => syncHistoryEntry("/detail", { replace: true })).not.toThrow();
  });
});
