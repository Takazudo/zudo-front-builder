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

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// @takazudo/zfb is a workspace dependency of @takazudo/zfb-runtime; the runtime
// island map (mounted/pending/capturedManifest) is a module-level singleton
// shared with router.ts, so importing it here exercises the same state the
// client-router mutates in production.
import {
  ISLAND_MOUNTED_ATTR,
  mountIslands,
  mountNewIslands,
  unmountIslands,
  type IslandManifestValue,
} from "@takazudo/zfb/runtime";
import type { RootHandle } from "@takazudo/zfb/zudo-react/client";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import { swapBodyElement } from "../../client-router/swap-functions.js";

const PERSIST = "data-zfb-transition-persist";
const BUILD = "0123456789abcdef";
const OLD_BUILD = "1111111111111111";
const NEW_BUILD = "2222222222222222";

const ownedAttrs = (build = BUILD): string =>
  `data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="${build}"`;

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
});
