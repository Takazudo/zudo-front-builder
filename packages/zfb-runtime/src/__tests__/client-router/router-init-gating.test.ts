/**
 * @vitest-environment happy-dom
 */
// The gating matrix for #2436 — router.ts's two module-scope side-effect blocks
// folded into init().
//
// The fold needs TWO once-guards with different eligibility rules, which is the
// most likely regression area:
//
//   - the page-state phase (history restore/seed + marking already-executed
//     scripts) runs on EVERY browser page, even a view-transition-ineligible
//     one, and must therefore run BEFORE init()'s eligibility early-return;
//   - the activation phase (popstate/load/pageshow/scroll + click/submit
//     listeners) is view-transition-gated and must NOT latch on an ineligible
//     call, so a later eligible init() still activates.
//
// Every case below loads a FRESH copy of router.ts via vi.resetModules(), since
// both guards latch for the lifetime of a module instance. `supportsViewTransitions`
// is captured at module-eval and happy-dom never defines document.startViewTransition,
// so eligibility here is driven entirely by the fallback meta: content="none"
// makes the page ineligible, its absence yields the "animate" default.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

vi.mock("@takazudo/zfb/runtime", () => ({
  mountNewIslands: vi.fn(),
  cancelPendingIslands: vi.fn(),
  unmountIslands: vi.fn(),
}));

installHappyDomShim();

// The fallback simulation path calls these from updateDOM(); happy-dom omits
// both. Only the navigate()-driven case below reaches them.
if (typeof document.getAnimations !== "function") {
  Object.defineProperty(document, "getAnimations", { configurable: true, value: () => [] });
}
if (typeof (globalThis as { KeyframeEffect?: unknown }).KeyframeEffect === "undefined") {
  (globalThis as { KeyframeEffect: unknown }).KeyframeEffect = class KeyframeEffect {};
}

type RouterModule = typeof import("../../client-router/router.js");

/**
 * Load a router instance whose two once-guards are both unlatched. Importing it
 * must itself be inert — that is case 1 below.
 */
async function freshRouter(): Promise<RouterModule> {
  vi.resetModules();
  return await import("../../client-router/router.js");
}

function addMeta(name: string, content: string): void {
  const meta = document.createElement("meta");
  meta.setAttribute("name", name);
  meta.setAttribute("content", content);
  document.head.appendChild(meta);
}

/** Opt the page into view transitions; optionally pin the fallback strategy. */
function primePage(options: { fallback?: "none" | "animate" | "swap" } = {}): void {
  resetDocument();
  addMeta("zfb-view-transitions-enabled", "true");
  if (options.fallback) addMeta("zfb-view-transitions-fallback", options.fallback);
}

function appendScript(): HTMLScriptElement {
  const script = document.createElement("script");
  script.textContent = `/* ${Math.random()} */`;
  document.body.appendChild(script);
  return script;
}

/** Listener-type names the router registered on window / document. */
function registeredTypes(spy: { mock: { calls: unknown[][] } }): string[] {
  return spy.mock.calls.map((c) => String(c[0]));
}

beforeEach(() => {
  primePage();
  // A known history entry per case; individual cases overwrite it.
  history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/gating-base");
});

afterEach(async () => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  await drainHappyDom();
});

describe("case 1 — importing router.ts is side-effect free", () => {
  it("registers no listener, writes no history entry, does not scroll, does not mark scripts", async () => {
    const script = appendScript();
    history.replaceState({ index: 5, scrollX: 0, scrollY: 400 }, "", "/gating-import");

    const windowAdd = vi.spyOn(window, "addEventListener");
    const documentAdd = vi.spyOn(document, "addEventListener");
    const replaceState = vi.spyOn(history, "replaceState");
    const pushState = vi.spyOn(history, "pushState");
    const scrollTo = vi.spyOn(window, "scrollTo");

    await freshRouter();

    expect(windowAdd).not.toHaveBeenCalled();
    expect(documentAdd).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    expect(pushState).not.toHaveBeenCalled();
    expect(scrollTo).not.toHaveBeenCalled();
    expect(script.dataset["zfbExec"]).toBeUndefined();
  });
});

describe("case 2 — an ineligible init() still seeds page state", () => {
  it("restores history state and marks scripts, but installs no activation listeners", async () => {
    primePage({ fallback: "none" });
    const script = appendScript();
    history.replaceState({ index: 9, scrollX: 0, scrollY: 250 }, "", "/gating-ineligible");

    const windowAdd = vi.spyOn(window, "addEventListener");
    const documentAdd = vi.spyOn(document, "addEventListener");
    const scrollTo = vi.spyOn(window, "scrollTo");

    const router = await freshRouter();
    router.init();

    // The seed half ran even though this page can never soft-navigate.
    expect(scrollTo).toHaveBeenCalledWith({ left: 0, top: 250 });
    expect(script.dataset["zfbExec"]).toBe("");

    // The activation half did not.
    expect(registeredTypes(windowAdd)).not.toContain("popstate");
    expect(registeredTypes(windowAdd)).not.toContain("pageshow");
    expect(registeredTypes(documentAdd)).not.toContain("click");
    expect(registeredTypes(documentAdd)).not.toContain("submit");
  });
});

describe("case 3 — an ineligible init() does not latch activation", () => {
  it("a later eligible init() still installs the listeners", async () => {
    primePage({ fallback: "none" });
    const router = await freshRouter();
    router.init();

    const windowAdd = vi.spyOn(window, "addEventListener");
    const documentAdd = vi.spyOn(document, "addEventListener");

    // The page becomes eligible (e.g. a second <ClientRouter /> mount without
    // fallback="none"): drop the opt-out meta and init() again.
    document.querySelector('[name="zfb-view-transitions-fallback"]')?.remove();
    router.init();

    expect(registeredTypes(windowAdd)).toContain("popstate");
    expect(registeredTypes(windowAdd)).toContain("load");
    expect(registeredTypes(windowAdd)).toContain("pageshow");
    expect(registeredTypes(documentAdd)).toContain("click");
    expect(registeredTypes(documentAdd)).toContain("submit");
  });
});

describe("case 4 — repeated init() calls repeat neither phase", () => {
  it("no second seed, no duplicate listeners", async () => {
    history.replaceState({ index: 2, scrollX: 0, scrollY: 60 }, "", "/gating-repeat");

    const router = await freshRouter();
    router.init();

    const windowAdd = vi.spyOn(window, "addEventListener");
    const documentAdd = vi.spyOn(document, "addEventListener");
    const scrollTo = vi.spyOn(window, "scrollTo");
    // A script appended after the first init(): a re-run of the seed phase
    // would mark it, so its dataset is the seed's second-run detector.
    const lateScript = appendScript();

    router.init();
    router.init();

    expect(scrollTo).not.toHaveBeenCalled();
    expect(lateScript.dataset["zfbExec"]).toBeUndefined();
    expect(registeredTypes(windowAdd)).not.toContain("popstate");
    expect(registeredTypes(documentAdd)).not.toContain("click");
  });
});

describe("case 5 — history.state present vs absent at init() time", () => {
  it("populated: adopts the entry's index and restores its scroll, writing no entry", async () => {
    history.replaceState({ index: 4, scrollX: 0, scrollY: 320 }, "", "/gating-populated");

    const scrollTo = vi.spyOn(window, "scrollTo");
    const replaceState = vi.spyOn(history, "replaceState");

    const router = await freshRouter();
    router.init();

    expect(scrollTo).toHaveBeenCalledWith({ left: 0, top: 320 });
    expect(replaceState).not.toHaveBeenCalled();

    // The adopted index is what the next router-managed push builds on: 4 → 5,
    // not the module default 0 → 1.
    router.syncHistoryEntry("/gating-populated-next");
    expect(history.state.index).toBe(5);
  });

  it("absent: seeds an entry via replaceState and pins scrollRestoration to manual", async () => {
    history.replaceState(null, "", "/gating-empty");
    expect(history.state).toBeNull();

    const scrollTo = vi.spyOn(window, "scrollTo");

    const router = await freshRouter();
    router.init();

    expect(history.state).toMatchObject({ index: 0 });
    expect(history.scrollRestoration).toBe("manual");
    // Seeding an entry is not a scroll restoration — the viewport stays put.
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("absent AND not opted into view transitions: no entry is seeded", async () => {
    resetDocument();
    history.replaceState(null, "", "/gating-no-optin");

    const replaceState = vi.spyOn(history, "replaceState");

    const router = await freshRouter();
    router.init();

    expect(replaceState).not.toHaveBeenCalled();
    expect(history.state).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// ensureNavigationState() — defensive seeding for the documented-unsupported
// init-less call path (#2436). navigate() and syncHistoryEntry() must not throw
// and must not stamp a fresh entry with the module-default index.
// ---------------------------------------------------------------------------
describe("init-less navigate() / syncHistoryEntry()", () => {
  it("syncHistoryEntry() adopts the page's existing index rather than resetting it", async () => {
    history.replaceState({ index: 7, scrollX: 0, scrollY: 0 }, "", "/unsupported-base");

    const router = await freshRouter();
    router.syncHistoryEntry("/unsupported-next");

    // 7 → 8. Before the defensive seed this stamped 1, silently breaking the
    // popstate direction calc for every later Back/Forward on the page.
    expect(history.state.index).toBe(8);
  });

  it("navigate() seeds originalLocation — the lifecycle event's `from` is never undefined", async () => {
    history.replaceState({ index: 0, scrollX: 0, scrollY: 0 }, "", "/nav-base");
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            `<!doctype html><html><head><meta name="zfb-view-transitions-enabled" content="true"><title>T</title></head><body><main>arrived</main></body></html>`,
            { headers: { "content-type": "text/html; charset=utf-8" } },
          ),
      ),
    );

    let from: URL | undefined;
    const onPrep = (ev: Event) => {
      from = (ev as unknown as { from: URL }).from;
    };
    document.addEventListener("zfb:before-preparation", onPrep);
    try {
      const router = await freshRouter();
      await router.navigate("/nav-target");
    } finally {
      document.removeEventListener("zfb:before-preparation", onPrep);
    }

    expect(from).toBeInstanceOf(URL);
    expect(from?.pathname).toBe("/nav-base");
    expect(document.querySelector("main")?.textContent).toBe("arrived");
  });
});
