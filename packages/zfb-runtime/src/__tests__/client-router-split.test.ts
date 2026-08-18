/**
 * @vitest-environment happy-dom
 */
// Behavioral guard for issue #2437 — the client-router component/activation
// split.
//
//   1. Importing the root barrel (`src/index.ts`) performs zero side
//      effects: no listeners registered, no history writes. Companion to
//      the source-text checks in `client-router-component-split.test.ts`.
//   2. The positive inverse: importing the `./client-router` subpath
//      barrel (the activation shim chain the islands bundler auto-injects
//      — see `crates/zfb-islands/src/esbuild.rs`) still calls `init()`
//      exactly once at module eval, i.e. activation stays byte-compatible.
//      Proven here as a cheap unit test rather than relying primarily on
//      the env-gated e2e for this contract.

import { afterEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./client-router/_helpers.js";

installHappyDomShim();

afterEach(async () => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.resetModules();
  await drainHappyDom();
});

describe("root barrel import — behavioral (#2437)", () => {
  it("importing the root barrel registers no listeners and writes no history", async () => {
    resetDocument();
    history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/purity-base");
    vi.resetModules();

    const windowAdd = vi.spyOn(window, "addEventListener");
    const documentAdd = vi.spyOn(document, "addEventListener");
    const replaceState = vi.spyOn(history, "replaceState");
    const pushState = vi.spyOn(history, "pushState");

    await import("../index.js");

    expect(windowAdd).not.toHaveBeenCalled();
    expect(documentAdd).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    expect(pushState).not.toHaveBeenCalled();
  });
});

describe("client-router subpath barrel (activation shim) — positive inverse (#2437)", () => {
  it("importing ./client-router calls init() exactly once at module eval", async () => {
    resetDocument();
    vi.resetModules();
    const initSpy = vi.fn();
    vi.doMock("../client-router/router.js", () => ({ init: initSpy }));

    await import("../client-router/index.js");

    expect(initSpy).toHaveBeenCalledTimes(1);

    vi.doUnmock("../client-router/router.js");
  });
});
