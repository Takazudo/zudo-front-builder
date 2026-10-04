/**
 * @vitest-environment happy-dom
 */
// End-to-end island-lifecycle test for the data-zfb-transition-persist contract
// (issue #1389). Unlike swap-functions.test.ts (which unit-tests the DOM lift in
// isolation) and zfb's runtime.test.ts (which unit-tests mount/unmount in
// isolation), this file drives the REAL cross-package sequence the router runs:
//
//   unmountIslands(oldBody, newBody)  →  swapBodyElement(newBody, oldBody)  →  mountNewIslands()
//
// using the real `swapBodyElement` from @takazudo/zfb-runtime AND the real
// owned island runtime (RootHandle lifecycle) from @takazudo/zfb. It proves that a
// persisted island's component instance actually survives the swap — the exact
// contract the pre-#1389 code silently violated (it unmounted every island,
// emptied the container, then re-hydrated against nothing).

import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

// @takazudo/zfb is a workspace dependency of @takazudo/zfb-runtime; the runtime
// island map (mounted/pending/capturedManifest) is a module-level singleton
// shared with router.ts, so importing it here exercises the same state the
// client-router mutates in production.
import {
  ISLAND_MOUNTED_ATTR,
  cancelPendingIslands,
  mountIslands,
  mountNewIslands,
  unmountIslands,
  type IslandManifestValue,
} from "@takazudo/zfb/runtime";
import type { RootHandle } from "@takazudo/zfb/zudo-react/client";
import { Island } from "@takazudo/zfb";
import { jsx } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { renderToString } from "@takazudo/zfb/zudo-react/server";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import { saveFocus, swapBodyElement } from "../../client-router/swap-functions.js";

const PERSIST = "data-zfb-transition-persist";
const BUILD = "0123456789abcdef";
const OLD_BUILD = "1111111111111111";
const NEW_BUILD = "2222222222222222";

const ownedAttrs = (build = BUILD): string =>
  `data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="${build}"`;

function SdkPersistedPanel({ value }: { value: number }) {
  return jsx("span", { children: value });
}

function rootHandle(
  component: string,
  build = BUILD,
  onDispose: () => void = () => {},
): RootHandle {
  let disposed = false;
  const handle: RootHandle = {
    protocol: "zudo-react/1",
    identity: { component, build },
    get disposed() {
      return disposed;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      onDispose();
    },
    // The runtime calls dispose() during swaps. These empty fixtures own no
    // child nodes, so unmount() has no separate DOM work to perform.
    unmount() {},
  };
  return handle;
}

const ownedEntry = (
  component: string,
  mount: IslandManifestValue["mount"],
  build = BUILD,
): IslandManifestValue => ({ identity: { component, build }, mount });

const incomingBody = (inner: string): HTMLElement =>
  new DOMParser().parseFromString(`<!doctype html><html><body>${inner}</body></html>`, "text/html")
    .body;

beforeEach(resetDocument);
afterEach(drainHappyDom);

describe("persist island lifecycle end-to-end (#1389)", () => {
  it("recreates a skip-SSR root when build identity changes, even with retained props", () => {
    document.body.innerHTML = `<div ${PERSIST}="modal" data-zfb-transition-persist-props="keep" data-zfb-island-skip-ssr="Modal" ${ownedAttrs(OLD_BUILD)} data-props="{}"></div>`;
    const el = document.body.firstElementChild!;
    const dispose = vi.fn();
    let activeBuild = OLD_BUILD;
    const mount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
      rootHandle("Modal", activeBuild, dispose),
    );
    mountIslands({ Modal: ownedEntry("Modal", mount, OLD_BUILD) });
    const next = incomingBody(
      `<div ${PERSIST}="modal" data-zfb-transition-persist-props="keep" data-zfb-island-skip-ssr="Modal" ${ownedAttrs(NEW_BUILD)} data-props='{"ignored":true}'></div>`,
    );
    unmountIslands(document.body, next);
    swapBodyElement(next, document.body);
    expect(el.getAttribute("data-zfb-build")).toBe(NEW_BUILD);
    expect(el.getAttribute("data-props")).toBe("{}");
    // The incoming page script installs its paired manifest before the router
    // re-walk. The old handle stays on the persisted node until remount.
    activeBuild = NEW_BUILD;
    mountIslands({ Modal: ownedEntry("Modal", mount, NEW_BUILD) });
    expect(mount).toHaveBeenCalledTimes(1);
    mountNewIslands();
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(mount).toHaveBeenCalledTimes(2);
    expect(mount.mock.calls[1]?.[2]).toBe("render");
  });

  it("remounts a renamed persisted island with the new component in render mode", () => {
    document.body.innerHTML = `<div ${PERSIST}="panel" data-zfb-island="Old" ${ownedAttrs()} data-props="{}"></div>`;
    const el = document.body.firstElementChild!;
    const dispose = vi.fn();
    const oldMount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
      rootHandle("Old", BUILD, dispose),
    );
    const newMount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
      rootHandle("New"),
    );
    mountIslands({ Old: ownedEntry("Old", oldMount), New: ownedEntry("New", newMount) });
    const next = incomingBody(
      `<div ${PERSIST}="panel" data-zfb-island="New" ${ownedAttrs()} data-props="{}"></div>`,
    );
    unmountIslands(document.body, next);
    swapBodyElement(next, document.body);
    expect(el.getAttribute("data-zfb-island")).toBe("New");
    mountNewIslands();
    expect(document.body.firstElementChild).toBe(el);
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(newMount).toHaveBeenCalledTimes(1);
    expect(newMount.mock.calls[0]?.[2]).toBe("render");
  });

  it("a persisted chrome island survives the real swap with its instance and node identity intact", () => {
    document.body.innerHTML = `
      <div ${PERSIST}="sidebar" data-zfb-island="Sidebar" ${ownedAttrs()} data-props='{"n":1}' data-when="load"></div>
      <div data-zfb-island="Toc" ${ownedAttrs()} data-props='{"p":1}' data-when="load"></div>
    `;
    const sidebarEl = document.querySelector(`[${PERSIST}="sidebar"]`)!;
    const tocEl = document.querySelector('[data-zfb-island="Toc"]')!;
    const sidebarDispose = vi.fn();
    const tocDispose = vi.fn();
    const sidebarMount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
      rootHandle("Sidebar", BUILD, sidebarDispose),
    );
    const tocMount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
      rootHandle("Toc", BUILD, tocDispose),
    );

    mountIslands({
      Sidebar: ownedEntry("Sidebar", sidebarMount),
      Toc: ownedEntry("Toc", tocMount),
    });
    expect(sidebarMount).toHaveBeenCalledTimes(1);
    expect(tocMount).toHaveBeenCalledTimes(1);
    expect(sidebarEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    expect(tocEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);

    // Same persist id + identical props for the sidebar; the Toc props differ but
    // Toc is not persisted, so it is discarded and remounted fresh.
    const newBody = incomingBody(`
      <div ${PERSIST}="sidebar" data-zfb-island="Sidebar" ${ownedAttrs()} data-props='{"n":1}' data-when="load"></div>
      <div data-zfb-island="Toc" ${ownedAttrs()} data-props='{"p":2}' data-when="load"></div>
    `);

    // The exact router sequence.
    unmountIslands(document.body, newBody);
    // The discarded island is cleaned up immediately, while the matching
    // persisted island keeps both its mounted entry and its public marker.
    expect(tocEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(false);
    expect(sidebarEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    swapBodyElement(newBody, document.body);
    mountNewIslands();

    // Node identity preserved through the whole swap — the lifted node IS the
    // original element, so all mounted component state rode along with it.
    const sidebarAfter = document.querySelector(`[${PERSIST}="sidebar"]`)!;
    expect(sidebarAfter).toBe(sidebarEl);
    expect(sidebarAfter.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    // Its instance was never torn down and never cold-remounted.
    expect(sidebarDispose).not.toHaveBeenCalled();
    expect(sidebarMount).toHaveBeenCalledTimes(1);
    // The non-persisted content island was unmounted with the old body and a
    // fresh instance mounted from the incoming markup.
    expect(tocDispose).toHaveBeenCalledTimes(1);
    expect(tocMount).toHaveBeenCalledTimes(2);
    expect(
      document.querySelector('[data-zfb-island="Toc"]')?.hasAttribute(ISLAND_MOUNTED_ATTR),
    ).toBe(true);
    // Surrounding DOM is the new page.
    expect(document.querySelector('[data-zfb-island="Toc"]')?.getAttribute("data-props")).toBe(
      '{"p":2}',
    );
  });

  it("a persisted island whose props changed is remounted with fresh props via the remount flag", () => {
    document.body.innerHTML = `
      <div ${PERSIST}="panel" data-zfb-island="Panel" ${ownedAttrs()} data-props='{"v":1}' data-when="load"></div>
    `;
    const panelEl = document.querySelector(`[${PERSIST}="panel"]`)!;
    const markerAtMount: boolean[] = [];
    const markerAtDispose: boolean[] = [];
    const dispose = vi.fn(() => {
      markerAtDispose.push(panelEl.hasAttribute(ISLAND_MOUNTED_ATTR));
    });
    const mount = vi.fn((...[_props, element]: Parameters<IslandManifestValue["mount"]>) => {
      markerAtMount.push(element.hasAttribute(ISLAND_MOUNTED_ATTR));
      return rootHandle("Panel", BUILD, dispose);
    });

    mountIslands({ Panel: ownedEntry("Panel", mount) });
    expect(mount).toHaveBeenCalledTimes(1);
    expect(mount.mock.calls[0]![0]).toEqual({ v: 1 });
    expect(markerAtMount).toEqual([false]);
    expect(panelEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);

    const newBody = incomingBody(`
      <div ${PERSIST}="panel" data-zfb-island="Panel" ${ownedAttrs()} data-props='{"v":2}' data-when="load"></div>
    `);

    unmountIslands(document.body, newBody);
    // The persisted node remains mounted until mountNewIslands consumes the
    // swap-functions remount flag.
    expect(panelEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    swapBodyElement(newBody, document.body);

    // swapBodyElement itself flagged the surviving node for remount and refreshed
    // its props — this is the cross-package signal, produced by the real code.
    expect(panelEl.hasAttribute("data-zfb-island-remount")).toBe(true);
    expect(panelEl.getAttribute("data-props")).toBe('{"v":2}');

    mountNewIslands();

    // The stale instance was cleaned up once, then a fresh one mounted with new props.
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(mount).toHaveBeenCalledTimes(2);
    expect(mount.mock.calls[1]![0]).toEqual({ v: 2 });
    expect(mount.mock.calls[1]![2]).toBe("render");
    expect(markerAtDispose).toEqual([true]);
    // clearMountedForRemount removed the marker before the forced callback;
    // fireInlineMount re-applied it after the replacement mount returned.
    expect(markerAtMount).toEqual([false, false]);
    expect(panelEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    // Flag consumed → remount fires exactly once; node identity still preserved.
    expect(panelEl.hasAttribute("data-zfb-island-remount")).toBe(false);
    expect(document.querySelector(`[${PERSIST}="panel"]`)).toBe(panelEl);
  });

  it("opting out of prop-copy keeps the instance and its old props without a remount", () => {
    // shouldCopyProps copies only when data-zfb-transition-persist-props is absent
    // or "false" (port-spec §4). Any other value (here "keep") suppresses the copy:
    // the persisted node keeps its live instance AND its original props.
    document.body.innerHTML = `
      <div ${PERSIST}="stateful" data-zfb-transition-persist-props="keep" data-zfb-island="Stateful" ${ownedAttrs()} data-props='{"v":1}' data-when="load"></div>
    `;
    const el = document.querySelector(`[${PERSIST}="stateful"]`)!;
    const dispose = vi.fn();
    const mount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
      rootHandle("Stateful", BUILD, dispose),
    );

    mountIslands({ Stateful: ownedEntry("Stateful", mount) });
    expect(mount).toHaveBeenCalledTimes(1);

    const newBody = incomingBody(`
      <div ${PERSIST}="stateful" data-zfb-transition-persist-props="keep" data-zfb-island="Stateful" ${ownedAttrs()} data-props='{"v":2}' data-when="load"></div>
    `);

    unmountIslands(document.body, newBody);
    swapBodyElement(newBody, document.body);

    // Props-copy suppressed → no remount flag, stale props retained on the node.
    expect(el.hasAttribute("data-zfb-island-remount")).toBe(false);
    expect(el.getAttribute("data-props")).toBe('{"v":1}');

    mountNewIslands();

    // Instance fully preserved: no unmount, no remount.
    expect(dispose).not.toHaveBeenCalled();
    expect(mount).toHaveBeenCalledTimes(1);
  });

  it("strips a stale mounted marker from incoming markup before mounting the island", () => {
    // Capture the manifest without mounting anything, so the incoming element
    // is genuinely unknown to this module instance's mounted map.
    document.body.innerHTML = "<p>old page</p>";
    const markerAtMount: boolean[] = [];
    const mount = vi.fn((...[_props, element]: Parameters<IslandManifestValue["mount"]>) => {
      markerAtMount.push(element.hasAttribute(ISLAND_MOUNTED_ATTR));
      return rootHandle("Fresh");
    });
    mountIslands({ Fresh: ownedEntry("Fresh", mount) });

    const newBody = incomingBody(
      `<div data-zfb-island="Fresh" ${ownedAttrs()} data-zfb-island-mounted data-props='{"fresh":true}'></div>`,
    );
    unmountIslands(document.body, newBody);
    swapBodyElement(newBody, document.body);

    const freshEl = document.querySelector('[data-zfb-island="Fresh"]')!;
    expect(freshEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);

    mountNewIslands();

    expect(mount).toHaveBeenCalledTimes(1);
    expect(markerAtMount).toEqual([false]);
    expect(freshEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
  });

  it("keeps SDK-generated persisted islands or remounts them according to their props option", () => {
    const zfbGlobal = globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild: string; zudoReactIslands: string[] };
    };
    const previousMetadata = zfbGlobal.__zfb;
    zfbGlobal.__zfb = { zudoReactBuild: BUILD, zudoReactIslands: ["SdkPersistedPanel"] };
    try {
      const sdkIsland = (key: string, value: number, persistProps = false) =>
        renderToString(
          Island({
            children: jsx(SdkPersistedPanel, { value }),
            persist: key,
            persistProps,
          }),
        );
      const page = (refreshValue: number, retainedValue: number) =>
        [
          sdkIsland("sdk-stable", 1),
          sdkIsland("sdk-refresh", refreshValue),
          sdkIsland("sdk-retain", retainedValue, true),
        ].join("");
      document.body.innerHTML = page(1, 1);

      const disposedElements: Element[] = [];
      const mount = vi.fn((...args: Parameters<IslandManifestValue["mount"]>) =>
        rootHandle("SdkPersistedPanel", BUILD, () => disposedElements.push(args[1])),
      );
      mountIslands({ SdkPersistedPanel: ownedEntry("SdkPersistedPanel", mount) });
      const nodes = Object.fromEntries(
        ["sdk-stable", "sdk-refresh", "sdk-retain"].map((key) => [
          key,
          document.querySelector(`[${PERSIST}="${key}"]`)!,
        ]),
      ) as Record<string, Element>;
      const stableHandle = (nodes["sdk-stable"] as unknown as Record<symbol, RootHandle>)[
        Symbol.for("@takazudo/zfb/zudo-react/root-v1")
      ];
      const refreshHandle = (nodes["sdk-refresh"] as unknown as Record<symbol, RootHandle>)[
        Symbol.for("@takazudo/zfb/zudo-react/root-v1")
      ];
      const retainedHandle = (nodes["sdk-retain"] as unknown as Record<symbol, RootHandle>)[
        Symbol.for("@takazudo/zfb/zudo-react/root-v1")
      ];

      const next = incomingBody(page(2, 2));
      cancelPendingIslands();
      unmountIslands(document.body, next);
      swapBodyElement(next, document.body);
      mountNewIslands();

      expect(document.querySelector(`[${PERSIST}="sdk-stable"]`)).toBe(nodes["sdk-stable"]);
      expect(document.querySelector(`[${PERSIST}="sdk-refresh"]`)).toBe(nodes["sdk-refresh"]);
      expect(document.querySelector(`[${PERSIST}="sdk-retain"]`)).toBe(nodes["sdk-retain"]);
      expect(
        (nodes["sdk-stable"] as unknown as Record<symbol, RootHandle>)[
          Symbol.for("@takazudo/zfb/zudo-react/root-v1")
        ],
      ).toBe(stableHandle);
      expect(
        (nodes["sdk-retain"] as unknown as Record<symbol, RootHandle>)[
          Symbol.for("@takazudo/zfb/zudo-react/root-v1")
        ],
      ).toBe(retainedHandle);
      expect(
        (nodes["sdk-refresh"] as unknown as Record<symbol, RootHandle>)[
          Symbol.for("@takazudo/zfb/zudo-react/root-v1")
        ],
      ).not.toBe(refreshHandle);
      expect(nodes["sdk-stable"]?.getAttribute("data-props")).toBe('{"value":1}');
      expect(nodes["sdk-refresh"]?.getAttribute("data-props")).toBe('{"value":2}');
      expect(nodes["sdk-retain"]?.getAttribute("data-props")).toBe('{"value":1}');
      expect(disposedElements).toEqual([nodes["sdk-refresh"]]);
      expect(mount).toHaveBeenCalledTimes(4);
      expect(mount.mock.calls[3]?.[0]).toEqual({ value: 2 });
      expect(mount.mock.calls[3]?.[2]).toBe("render");
    } finally {
      if (previousMetadata === undefined) delete zfbGlobal.__zfb;
      else zfbGlobal.__zfb = previousMetadata;
    }
  });

  it.each(["idle", "visible", "media"] as const)(
    "reschedules a persisted SDK %s island after navigation and ignores its stale callback",
    (when) => {
      const zfbGlobal = globalThis as typeof globalThis & {
        __zfb?: { zudoReactBuild: string; zudoReactIslands: string[] };
      };
      const previousMetadata = zfbGlobal.__zfb;
      zfbGlobal.__zfb = { zudoReactBuild: BUILD, zudoReactIslands: ["SdkPersistedPanel"] };
      const callbacks: Array<() => void> = [];
      try {
        if (when === "idle") {
          vi.stubGlobal("requestIdleCallback", (callback: () => void) => {
            callbacks.push(callback);
            return callbacks.length;
          });
          vi.stubGlobal("cancelIdleCallback", vi.fn());
        } else if (when === "visible") {
          class TestObserver {
            constructor(
              private readonly callback: (entries: unknown[], observer: TestObserver) => void,
            ) {}
            private target: Element | undefined;
            observe(target: Element) {
              this.target = target;
              callbacks.push(() =>
                this.callback([{ isIntersecting: true, target: this.target }], this),
              );
            }
            disconnect() {}
          }
          vi.stubGlobal("IntersectionObserver", TestObserver);
        } else {
          vi.stubGlobal("matchMedia", (query: string) => {
            let listener: ((event: { matches: boolean }) => void) | undefined;
            const entry = () => listener?.({ matches: true });
            callbacks.push(entry);
            return {
              matches: false,
              media: query,
              addEventListener(_type: string, next: (event: { matches: boolean }) => void) {
                listener = next;
              },
              // Keep the callback callable to model a match event already
              // queued when the router removes its listener during a swap.
              removeEventListener() {},
            };
          });
        }

        const markup = renderToString(
          Island({
            children: jsx(SdkPersistedPanel, { value: 7 }),
            when,
            ...(when === "media" ? { media: "(min-width: 40rem)" } : {}),
            persist: `sdk-${when}`,
            ...(when === "media" ? { ssrFallback: jsx("i", { children: "pending" }) } : {}),
          }),
        );
        document.body.innerHTML = markup;
        const dispose = vi.fn();
        const mount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
          rootHandle("SdkPersistedPanel", BUILD, dispose),
        );
        mountIslands({ SdkPersistedPanel: ownedEntry("SdkPersistedPanel", mount) });
        expect(mount).not.toHaveBeenCalled();
        expect(callbacks).toHaveLength(1);

        const next = incomingBody(markup);
        cancelPendingIslands();
        unmountIslands(document.body, next);
        swapBodyElement(next, document.body);
        mountNewIslands();

        expect(callbacks).toHaveLength(2);
        callbacks[0]!();
        expect(mount).not.toHaveBeenCalled();
        callbacks[1]!();
        expect(mount).toHaveBeenCalledTimes(1);
        expect(mount.mock.calls[0]?.[2]).toBe(when === "media" ? "render" : "hydrate");
        expect(document.querySelector(`[${PERSIST}="sdk-${when}"]`)?.isConnected).toBe(true);
        callbacks[1]!();
        expect(mount).toHaveBeenCalledTimes(1);
        expect(dispose).not.toHaveBeenCalled();
      } finally {
        vi.unstubAllGlobals();
        if (previousMetadata === undefined) delete zfbGlobal.__zfb;
        else zfbGlobal.__zfb = previousMetadata;
      }
    },
  );
});

// These tests exercise islands riding inside retained non-island chrome. Keep
// both DOM move implementations in the matrix: moveBefore is optional in browsers.
for (const moveMode of ["moveBefore", "fallback"] as const) {
  describe(`descendant island lifecycle (${moveMode})`, () => {
    const originalMoveBefore = Object.getOwnPropertyDescriptor(Element.prototype, "moveBefore");
    beforeEach(() => {
      if (moveMode === "moveBefore") {
        Object.defineProperty(Element.prototype, "moveBefore", {
          configurable: true,
          value(this: Element, node: Node, child: Node | null) {
            this.insertBefore(node, child);
          },
        });
      } else {
        Object.defineProperty(Element.prototype, "moveBefore", {
          configurable: true,
          value: undefined,
        });
      }
    });
    afterEach(() => {
      if (originalMoveBefore)
        Object.defineProperty(Element.prototype, "moveBefore", originalMoveBefore);
      else delete (Element.prototype as Element & { moveBefore?: unknown }).moveBefore;
      vi.unstubAllGlobals();
    });

    const island = (name: string, props = 1, extra = "") => {
      const when = extra.includes("data-when=") ? "" : 'data-when="load"';
      return `<div data-zfb-island="${name}" ${ownedAttrs()} data-props='{"v":${props}}' ${when} ${extra}></div>`;
    };
    const header = (contents: string) => `<header ${PERSIST}="h">${contents}</header>`;
    const handleOf = (el: Element): RootHandle | undefined =>
      (el as unknown as Record<symbol, RootHandle | undefined>)[
        Symbol.for("@takazudo/zfb/zudo-react/root-v1")
      ];
    function ledger(...names: string[]) {
      const mounts = new Map<string, ReturnType<typeof vi.fn>>();
      const disposes = new Map<string, ReturnType<typeof vi.fn>>();
      const manifest: Record<string, IslandManifestValue> = {};
      for (const name of names) {
        const dispose = vi.fn();
        const mount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
          rootHandle(name, BUILD, dispose),
        );
        disposes.set(name, dispose);
        mounts.set(name, mount);
        manifest[name] = ownedEntry(name, mount);
      }
      return {
        manifest,
        mount: (name: string) => mounts.get(name)!,
        dispose: (name: string) => disposes.get(name)!,
      };
    }
    function navigate(markup: string) {
      const next = incomingBody(markup);
      cancelPendingIslands();
      unmountIslands(document.body, next);
      swapBodyElement(next, document.body);
      mountNewIslands();
    }

    it("keeps the node and handle through A→B→A", () => {
      document.body.innerHTML = header(island("A")) + "<main>page A</main>";
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      const handle = handleOf(el);
      navigate(header(island("A")) + "<main>page B</main>");
      expect(document.querySelector("main")?.textContent).toBe("page B");
      navigate(header(island("A")) + "<main>page A</main>");
      expect(document.querySelector("main")?.textContent).toBe("page A");
      expect(document.querySelector('[data-zfb-island="A"]')).toBe(el);
      expect(handleOf(el)).toBe(handle);
      expect(el.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
      expect(l.mount("A")).toHaveBeenCalledTimes(1);
      expect(l.dispose("A")).not.toHaveBeenCalled();
    });

    it("copies changed props and remounts once in render mode", () => {
      document.body.innerHTML = header(island("A"));
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      navigate(header(island("A", 2)));
      expect(document.querySelector('[data-zfb-island="A"]')).toBe(el);
      expect(el.getAttribute("data-props")).toBe('{"v":2}');
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(l.mount("A")).toHaveBeenCalledTimes(2);
      expect(l.mount("A").mock.calls[1]?.[0]).toEqual({ v: 2 });
      expect(l.mount("A").mock.calls[1]?.[2]).toBe("render");
      expect(handleOf(el)).not.toBe(l.mount("A").mock.results[0]?.value);
    });

    it("copies a changed build and mounts against the new manifest identity", () => {
      document.body.innerHTML = header(island("A"));
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      const next = incomingBody(header(island("A").replace(BUILD, NEW_BUILD)));
      cancelPendingIslands();
      unmountIslands(document.body, next);
      swapBodyElement(next, document.body);
      const newMount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
        rootHandle("A", NEW_BUILD),
      );
      mountIslands({ A: ownedEntry("A", newMount, NEW_BUILD) });
      mountNewIslands();
      expect(el.getAttribute("data-zfb-build")).toBe(NEW_BUILD);
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(newMount).toHaveBeenCalledTimes(1);
      expect(newMount.mock.calls[0]?.[2]).toBe("render");
    });

    it("disposes and detaches a removed descendant", () => {
      document.body.innerHTML = header(island("A"));
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      navigate(header("<p>empty</p>"));
      expect(el.isConnected).toBe(false);
      expect(handleOf(el)).toBeUndefined();
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(l.mount("A")).toHaveBeenCalledTimes(1);
    });

    it("pairs same-component siblings by order and remounts only the changed one", () => {
      document.body.innerHTML = header(island("A", 1) + island("A", 2));
      const l = ledger("A");
      mountIslands(l.manifest);
      const [first, second] = [...document.querySelectorAll('[data-zfb-island="A"]')];
      const firstHandle = handleOf(first!);
      navigate(header(island("A", 1) + island("A", 3)));
      expect([...document.querySelectorAll('[data-zfb-island="A"]')]).toEqual([first, second]);
      expect(handleOf(first!)).toBe(firstHandle);
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(l.mount("A")).toHaveBeenCalledTimes(3);
      expect(l.mount("A").mock.calls[2]?.[0]).toEqual({ v: 3 });
      expect(l.mount("A").mock.calls[2]?.[2]).toBe("render");
    });

    it("ignores an inserted same-component sibling inside retained chrome", () => {
      document.body.innerHTML = header(island("A", 1) + island("A", 2));
      const l = ledger("A");
      mountIslands(l.manifest);
      const original = [...document.querySelectorAll('[data-zfb-island="A"]')];
      const handles = original.map(handleOf);
      navigate(header(island("A", 1) + island("A", 2) + island("A", 3)));
      expect([...document.querySelectorAll('[data-zfb-island="A"]')]).toEqual(original);
      expect(original.map(handleOf)).toEqual(handles);
      expect(l.mount("A")).toHaveBeenCalledTimes(2);
      expect(l.dispose("A")).not.toHaveBeenCalled();
    });

    it("removes only the unmatched same-component sibling", () => {
      document.body.innerHTML = header(island("A", 1) + island("A", 2));
      const l = ledger("A");
      mountIslands(l.manifest);
      const [first, second] = [...document.querySelectorAll('[data-zfb-island="A"]')];
      const handle = handleOf(first!);
      navigate(header(island("A", 1)));
      expect(document.querySelectorAll('[data-zfb-island="A"]')).toHaveLength(1);
      expect(document.querySelector('[data-zfb-island="A"]')).toBe(first);
      expect(handleOf(first!)).toBe(handle);
      expect(second!.isConnected).toBe(false);
      expect(handleOf(second!)).toBeUndefined();
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(l.mount("A")).toHaveBeenCalledTimes(2);
    });

    it("replaces a component at the same slot in render mode", () => {
      document.body.innerHTML = header(island("A"));
      const l = ledger("A", "B");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      navigate(header(island("B")));
      expect(document.querySelector('[data-zfb-island="B"]')).toBe(el);
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(l.mount("B")).toHaveBeenCalledTimes(1);
      expect(l.mount("B").mock.calls[0]?.[2]).toBe("render");
    });

    for (const variant of ["unchanged", "changed", "removed"] as const) {
      it(`cancels stale idle callbacks for a ${variant} descendant`, () => {
        const callbacks = new Map<number, IdleRequestCallback>();
        let id = 0;
        const cancelled = vi.fn((handle: number) => {
          callbacks.delete(handle);
        });
        vi.stubGlobal("requestIdleCallback", (cb: IdleRequestCallback) => {
          callbacks.set(++id, cb);
          return id;
        });
        vi.stubGlobal("cancelIdleCallback", cancelled);
        document.body.innerHTML = header(island("A", 1, 'data-when="idle"'));
        const l = ledger("A");
        mountIslands(l.manifest);
        expect(l.mount("A")).not.toHaveBeenCalled();
        const stale = [...callbacks.values()][0]!;
        const next =
          variant === "removed"
            ? header("<p>gone</p>")
            : header(island("A", variant === "changed" ? 2 : 1, 'data-when="idle"'));
        navigate(next);
        expect(cancelled).toHaveBeenCalledTimes(1);
        // A changed descendant forces an immediate render mount. The cancelled
        // pre-swap callback must never add a second mount.
        const mountsAfterSwap = l.mount("A").mock.calls.length;
        stale({ didTimeout: false, timeRemaining: () => 50 });
        expect(l.mount("A")).toHaveBeenCalledTimes(mountsAfterSwap);
        for (const cb of callbacks.values()) cb({ didTimeout: false, timeRemaining: () => 50 });
        expect(l.mount("A")).toHaveBeenCalledTimes(variant === "removed" ? 0 : 1);
        if (variant !== "removed") {
          expect(l.mount("A").mock.calls[0]?.[2]).toBe(
            variant === "changed" ? "render" : "hydrate",
          );
          expect(l.mount("A").mock.calls[0]?.[0]).toEqual({ v: variant === "changed" ? 2 : 1 });
        }
      });
    }

    it("retains islands in both nested persist boundaries without double disposal", () => {
      const markup = header(island("A") + `<aside ${PERSIST}="inner">${island("B")}</aside>`);
      document.body.innerHTML = markup;
      const l = ledger("A", "B");
      mountIslands(l.manifest);
      const nodes = [...document.querySelectorAll("[data-zfb-island]")];
      const handles = nodes.map(handleOf);
      navigate(markup);
      expect([...document.querySelectorAll("[data-zfb-island]")]).toEqual(nodes);
      expect(nodes.map(handleOf)).toEqual(handles);
      expect(nodes.every((node) => node.isConnected)).toBe(true);
      expect(l.dispose("A")).not.toHaveBeenCalled();
      expect(l.dispose("B")).not.toHaveBeenCalled();
    });

    it("retains C inside P when the incoming C target moves under Q", () => {
      document.body.innerHTML = header(
        `<div ${PERSIST}="P"><aside ${PERSIST}="C">${island("A")}</aside></div><div ${PERSIST}="Q"></div>`,
      );
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      const handle = handleOf(el);
      navigate(
        header(
          `<div ${PERSIST}="P"></div><div ${PERSIST}="Q"><aside ${PERSIST}="C">${island("A")}</aside></div>`,
        ),
      );
      expect(el.isConnected).toBe(true);
      expect(handleOf(el)).toBe(handle);
      expect(l.dispose("A")).not.toHaveBeenCalled();
      expect(l.mount("A")).toHaveBeenCalledTimes(1);
    });

    it("disposes an island in a nested persist node that is not retained", () => {
      document.body.innerHTML = header(`<aside ${PERSIST}="lost">${island("A")}</aside>`);
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      navigate(header("<p>lost aside absent</p>"));
      expect(el.isConnected).toBe(false);
      expect(handleOf(el)).toBeUndefined();
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
    });

    it("restores focus and selection inside a kept descendant", () => {
      document.body.innerHTML = header(
        island("A").replace("</div>", '<input value="abcdef"></div>'),
      );
      const l = ledger("A");
      mountIslands(l.manifest);
      const input = document.querySelector("input")!;
      input.focus();
      input.setSelectionRange(2, 4);
      const next = incomingBody(
        header(island("A").replace("</div>", '<input value="abcdef"></div>')),
      );
      cancelPendingIslands();
      unmountIslands(document.body, next);
      const restore = saveFocus();
      swapBodyElement(next, document.body);
      restore();
      mountNewIslands();
      expect(document.activeElement).toBe(input);
      expect([input.selectionStart, input.selectionEnd]).toEqual([2, 4]);
      expect(l.dispose("A")).not.toHaveBeenCalled();
    });

    it("honours a consumer-set remount flag before unmount", () => {
      document.body.innerHTML = header(island("A"));
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      el.setAttribute("data-zfb-island-remount", "");
      navigate(header(island("A")));
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(l.mount("A")).toHaveBeenCalledTimes(2);
      expect(l.mount("A").mock.calls[1]?.[2]).toBe("render");
      expect(el.hasAttribute("data-zfb-island-remount")).toBe(false);
    });

    it("remounts a changed identity while persist-props=true retains old props", () => {
      document.body.innerHTML = header(island("A", 1, 'data-zfb-transition-persist-props="true"'));
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      const oldHandle = handleOf(el);
      const next = incomingBody(
        header(
          island("A", 2, 'data-zfb-transition-persist-props="true"').replace(BUILD, NEW_BUILD),
        ),
      );
      cancelPendingIslands();
      unmountIslands(document.body, next);
      swapBodyElement(next, document.body);
      const newMount = vi.fn((..._args: Parameters<IslandManifestValue["mount"]>) =>
        rootHandle("A", NEW_BUILD),
      );
      mountIslands({ A: ownedEntry("A", newMount, NEW_BUILD) });
      mountNewIslands();
      expect(document.querySelector('[data-zfb-island="A"]')).toBe(el);
      expect(el.getAttribute("data-zfb-build")).toBe(NEW_BUILD);
      expect(el.getAttribute("data-props")).toBe('{"v":1}');
      expect(l.dispose("A")).toHaveBeenCalledTimes(1);
      expect(newMount).toHaveBeenCalledTimes(1);
      expect(newMount.mock.calls[0]?.[0]).toEqual({ v: 1 });
      expect(newMount.mock.calls[0]?.[2]).toBe("render");
      expect(handleOf(el)).not.toBe(oldHandle);
    });

    it("keeps old props and handle with persist-props=true on a nested island", () => {
      document.body.innerHTML = header(island("A", 1, 'data-zfb-transition-persist-props="true"'));
      const l = ledger("A");
      mountIslands(l.manifest);
      const el = document.querySelector('[data-zfb-island="A"]')!;
      const handle = handleOf(el);
      navigate(header(island("A", 2, 'data-zfb-transition-persist-props="true"')));
      expect(el.getAttribute("data-props")).toBe('{"v":1}');
      expect(handleOf(el)).toBe(handle);
      expect(l.dispose("A")).not.toHaveBeenCalled();
      expect(l.mount("A")).toHaveBeenCalledTimes(1);
    });
  });
}
