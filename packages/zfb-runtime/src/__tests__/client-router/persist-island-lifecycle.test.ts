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
// island runtime (mount/unmount map) from @takazudo/zfb. It proves that a
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
} from "@takazudo/zfb/runtime";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import { swapBodyElement } from "../../client-router/swap-functions.js";

const PERSIST = "data-zfb-transition-persist";

const incomingBody = (inner: string): HTMLElement =>
  new DOMParser().parseFromString(`<!doctype html><html><body>${inner}</body></html>`, "text/html")
    .body;

beforeEach(resetDocument);
afterEach(drainHappyDom);

describe("persist island lifecycle end-to-end (#1389)", () => {
  it("a persisted chrome island survives the real swap with its instance and node identity intact", () => {
    document.body.innerHTML = `
      <div ${PERSIST}="sidebar" data-zfb-island="Sidebar" data-props='{"n":1}' data-when="load"></div>
      <div data-zfb-island="Toc" data-props='{"p":1}' data-when="load"></div>
    `;
    const sidebarEl = document.querySelector(`[${PERSIST}="sidebar"]`)!;
    const tocEl = document.querySelector('[data-zfb-island="Toc"]')!;
    const sidebarMount = vi.fn();
    const sidebarUnmount = vi.fn();
    const tocMount = vi.fn();
    const tocUnmount = vi.fn();

    mountIslands({
      Sidebar: { mount: sidebarMount, unmount: sidebarUnmount },
      Toc: { mount: tocMount, unmount: tocUnmount },
    });
    expect(sidebarMount).toHaveBeenCalledTimes(1);
    expect(tocMount).toHaveBeenCalledTimes(1);
    expect(sidebarEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    expect(tocEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);

    // Same persist id + identical props for the sidebar; the Toc props differ but
    // Toc is not persisted, so it is discarded and remounted fresh.
    const newBody = incomingBody(`
      <div ${PERSIST}="sidebar" data-zfb-island="Sidebar" data-props='{"n":1}' data-when="load"></div>
      <div data-zfb-island="Toc" data-props='{"p":2}' data-when="load"></div>
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
    // original element, so all internal Preact/React state rode along with it.
    const sidebarAfter = document.querySelector(`[${PERSIST}="sidebar"]`)!;
    expect(sidebarAfter).toBe(sidebarEl);
    expect(sidebarAfter.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    // Its instance was never torn down and never cold-remounted.
    expect(sidebarUnmount).not.toHaveBeenCalled();
    expect(sidebarMount).toHaveBeenCalledTimes(1);
    // The non-persisted content island was unmounted with the old body and a
    // fresh instance mounted from the incoming markup.
    expect(tocUnmount).toHaveBeenCalledTimes(1);
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
      <div ${PERSIST}="panel" data-zfb-island="Panel" data-props='{"v":1}' data-when="load"></div>
    `;
    const panelEl = document.querySelector(`[${PERSIST}="panel"]`)!;
    const markerAtMount: boolean[] = [];
    const mount = vi.fn((_props: Record<string, unknown>, element: Element) => {
      markerAtMount.push(element.hasAttribute(ISLAND_MOUNTED_ATTR));
    });
    const markerAtUnmount: boolean[] = [];
    const unmount = vi.fn((element: Element) => {
      markerAtUnmount.push(element.hasAttribute(ISLAND_MOUNTED_ATTR));
    });

    mountIslands({ Panel: { mount, unmount } });
    expect(mount).toHaveBeenCalledTimes(1);
    expect(mount.mock.calls[0]![0]).toEqual({ v: 1 });
    expect(markerAtMount).toEqual([false]);
    expect(panelEl.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);

    const newBody = incomingBody(`
      <div ${PERSIST}="panel" data-zfb-island="Panel" data-props='{"v":2}' data-when="load"></div>
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
    expect(unmount).toHaveBeenCalledTimes(1);
    expect(mount).toHaveBeenCalledTimes(2);
    expect(mount.mock.calls[1]![0]).toEqual({ v: 2 });
    expect(markerAtUnmount).toEqual([true]);
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
      <div ${PERSIST}="stateful" data-zfb-transition-persist-props="keep" data-zfb-island="Stateful" data-props='{"v":1}' data-when="load"></div>
    `;
    const el = document.querySelector(`[${PERSIST}="stateful"]`)!;
    const mount = vi.fn();
    const unmount = vi.fn();

    mountIslands({ Stateful: { mount, unmount } });
    expect(mount).toHaveBeenCalledTimes(1);

    const newBody = incomingBody(`
      <div ${PERSIST}="stateful" data-zfb-transition-persist-props="keep" data-zfb-island="Stateful" data-props='{"v":2}' data-when="load"></div>
    `);

    unmountIslands(document.body, newBody);
    swapBodyElement(newBody, document.body);

    // Props-copy suppressed → no remount flag, stale props retained on the node.
    expect(el.hasAttribute("data-zfb-island-remount")).toBe(false);
    expect(el.getAttribute("data-props")).toBe('{"v":1}');

    mountNewIslands();

    // Instance fully preserved: no unmount, no remount.
    expect(unmount).not.toHaveBeenCalled();
    expect(mount).toHaveBeenCalledTimes(1);
  });

  it("strips a stale mounted marker from incoming markup before mounting the island", () => {
    // Capture the manifest without mounting anything, so the incoming element
    // is genuinely unknown to this module instance's mounted map.
    document.body.innerHTML = "<p>old page</p>";
    const markerAtMount: boolean[] = [];
    const mount = vi.fn((_props: Record<string, unknown>, element: Element) => {
      markerAtMount.push(element.hasAttribute(ISLAND_MOUNTED_ATTR));
    });
    mountIslands({ Fresh: { mount } });

    const newBody = incomingBody(
      `<div data-zfb-island="Fresh" data-zfb-island-mounted data-props='{"fresh":true}'></div>`,
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
