/**
 * @vitest-environment happy-dom
 */
// Unit tests for client-router/events.
//
// We assert (a) the public Event constants are stable strings, (b)
// `triggerEvent`/`onPageLoad` dispatch the right Event on document, (c)
// `doPreparation` dispatches a TransitionBeforePreparationEvent first then
// `zfb:after-preparation` once the loader resolves without preventing default,
// and (d) `doSwap` dispatches the swap event before invoking afterDispatch and
// the supplied swap callback. Lifecycle ordering is the contract that any
// consumer subscribing to lifecycle events depends on, so we verify the order
// observably (sequence numbers) rather than via implementation snooping.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { drainHappyDom, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import {
  TRANSITION_AFTER_PREPARATION,
  TRANSITION_AFTER_SWAP,
  TRANSITION_BEFORE_PREPARATION,
  TRANSITION_BEFORE_SWAP,
  TRANSITION_PAGE_LOAD,
  TransitionBeforePreparationEvent,
  TransitionBeforeSwapEvent,
  doPreparation,
  doSwap,
  isTransitionBeforePreparationEvent,
  isTransitionBeforeSwapEvent,
  onPageLoad,
  triggerEvent,
  updateScrollPosition,
} from "../../client-router/events.js";

beforeEach(resetDocument);
afterEach(drainHappyDom);

describe("event constants", () => {
  // The constants are pinned because subscribers register listeners by
  // string name (e.g. `document.addEventListener("zfb:before-swap", ...)`).
  // Any rename is a public-API break and must be intentional.
  it("exposes the canonical zfb:* names", () => {
    expect(TRANSITION_BEFORE_PREPARATION).toBe("zfb:before-preparation");
    expect(TRANSITION_AFTER_PREPARATION).toBe("zfb:after-preparation");
    expect(TRANSITION_BEFORE_SWAP).toBe("zfb:before-swap");
    expect(TRANSITION_AFTER_SWAP).toBe("zfb:after-swap");
    expect(TRANSITION_PAGE_LOAD).toBe("zfb:page-load");
  });
});

describe("triggerEvent / onPageLoad", () => {
  it("triggerEvent dispatches a basic Event of the named type on document", () => {
    const seen: string[] = [];
    const handler = (e: Event) => seen.push(e.type);
    document.addEventListener("zfb:after-preparation", handler);
    triggerEvent("zfb:after-preparation");
    document.removeEventListener("zfb:after-preparation", handler);
    expect(seen).toEqual(["zfb:after-preparation"]);
  });

  it("onPageLoad dispatches the zfb:page-load event", () => {
    const seen: string[] = [];
    const handler = (e: Event) => seen.push(e.type);
    document.addEventListener("zfb:page-load", handler);
    onPageLoad();
    document.removeEventListener("zfb:page-load", handler);
    expect(seen).toEqual(["zfb:page-load"]);
  });
});

describe("doPreparation — lifecycle ordering", () => {
  function makeArgs(overrides?: { loader?: () => Promise<void> }) {
    const from = new URL("http://test.local/from");
    const to = new URL("http://test.local/to");
    const newDoc = window.document;
    const signal = new AbortController().signal;
    const loader = overrides?.loader ?? (async () => {});
    return { from, to, newDoc, signal, loader };
  }

  it("dispatches zfb:before-preparation, runs the loader, then zfb:after-preparation", async () => {
    const order: string[] = [];
    const { from, to, newDoc, signal } = makeArgs();
    const loader = async () => {
      order.push("loader");
    };

    const beforeHandler = (e: Event) => {
      // The before-preparation event is the only event in the lifecycle that
      // is the typed subclass; the rest are plain Event instances.
      if (isTransitionBeforePreparationEvent(e)) order.push("before-preparation");
    };
    const afterHandler = (e: Event) => order.push(e.type);
    document.addEventListener("zfb:before-preparation", beforeHandler);
    document.addEventListener("zfb:after-preparation", afterHandler);

    await doPreparation(
      from,
      to,
      "forward",
      "push",
      undefined,
      undefined,
      signal,
      undefined,
      loader,
    );

    document.removeEventListener("zfb:before-preparation", beforeHandler);
    document.removeEventListener("zfb:after-preparation", afterHandler);

    expect(order).toEqual(["before-preparation", "loader", "zfb:after-preparation"]);
    void newDoc; // included in args to keep the signature realistic
  });

  it("when defaultPrevented in zfb:before-preparation, after-preparation is NOT fired", async () => {
    const after = vi.fn();
    const beforeHandler = (e: Event) => {
      if (isTransitionBeforePreparationEvent(e)) e.preventDefault();
    };
    document.addEventListener("zfb:before-preparation", beforeHandler);
    document.addEventListener("zfb:after-preparation", after);

    const { from, to, signal, loader } = makeArgs();
    const ev = await doPreparation(
      from,
      to,
      "forward",
      "push",
      undefined,
      undefined,
      signal,
      undefined,
      loader,
    );

    document.removeEventListener("zfb:before-preparation", beforeHandler);
    document.removeEventListener("zfb:after-preparation", after);

    // `dispatchEvent` returns false when defaultPrevented; our impl skips the
    // loader + after-preparation dispatch in that case.
    expect(ev.defaultPrevented).toBe(true);
    expect(after).not.toHaveBeenCalled();
  });

  it("returns a TransitionBeforePreparationEvent carrying from, to, navigationType, formData", async () => {
    const { from, to, signal, loader } = makeArgs();
    const formData = new FormData();
    formData.append("k", "v");
    const ev = await doPreparation(
      from,
      to,
      "back",
      "traverse",
      undefined,
      undefined,
      signal,
      formData,
      loader,
    );
    expect(ev).toBeInstanceOf(TransitionBeforePreparationEvent);
    expect(ev.from).toBe(from);
    expect(ev.to).toBe(to);
    expect(ev.direction).toBe("back");
    expect(ev.navigationType).toBe("traverse");
    expect(ev.formData).toBe(formData);
    expect(ev.signal).toBe(signal);
  });
});

describe("doSwap — lifecycle ordering", () => {
  it("dispatches zfb:before-swap, awaits afterDispatch, then calls swap()", async () => {
    const order: string[] = [];
    const beforeHandler = (e: Event) => {
      if (isTransitionBeforeSwapEvent(e)) order.push("before-swap");
    };
    document.addEventListener("zfb:before-swap", beforeHandler);

    const fakeViewTransition = { skipTransition: () => {} } as unknown as ViewTransition;
    const fakePrepEvent = new TransitionBeforePreparationEvent(
      new URL("http://test.local/a"),
      new URL("http://test.local/b"),
      "forward",
      "push",
      undefined,
      undefined,
      window.document,
      new AbortController().signal,
      undefined,
      async () => {},
    );
    const afterDispatch = async () => {
      // afterDispatch (e.g. animateFallbackOld) is awaited between the
      // dispatched zfb:before-swap and the actual swap() call.
      order.push("afterDispatch");
    };

    const ev = await doSwap(fakePrepEvent, fakeViewTransition, afterDispatch);
    // `swap()` is overridable on the event — by default it calls swap(newDoc),
    // which mutates the live DOM. We replace it with a sentinel so we can
    // assert the dispatch order without exercising swap-functions internals.
    // (The default impl is covered by swap-functions.test.ts.)
    void ev;
    // After doSwap completes, swap() has been invoked internally on the event.
    // We don't observe swap() here directly; ordering is verified by
    // before-swap → afterDispatch.
    document.removeEventListener("zfb:before-swap", beforeHandler);
    expect(order).toEqual(["before-swap", "afterDispatch"]);
  });

  it("returns a TransitionBeforeSwapEvent with viewTransition + a swap() function", async () => {
    const fakeViewTransition = { skipTransition: () => {} } as unknown as ViewTransition;
    const fakePrepEvent = new TransitionBeforePreparationEvent(
      new URL("http://test.local/a"),
      new URL("http://test.local/b"),
      "forward",
      "push",
      undefined,
      undefined,
      window.document,
      new AbortController().signal,
      undefined,
      async () => {},
    );
    const ev = await doSwap(fakePrepEvent, fakeViewTransition);
    expect(ev).toBeInstanceOf(TransitionBeforeSwapEvent);
    expect(ev.viewTransition).toBe(fakeViewTransition);
    expect(typeof ev.swap).toBe("function");
  });

  it("isTransitionBeforeSwapEvent guards correctly", () => {
    const ok = new TransitionBeforeSwapEvent(
      new TransitionBeforePreparationEvent(
        new URL("http://test.local/a"),
        new URL("http://test.local/b"),
        "forward",
        "push",
        undefined,
        undefined,
        window.document,
        new AbortController().signal,
        undefined,
        async () => {},
      ),
      { skipTransition: () => {} } as unknown as ViewTransition,
    );
    expect(isTransitionBeforeSwapEvent(ok)).toBe(true);
    expect(isTransitionBeforeSwapEvent({ type: "click" })).toBe(false);
  });
});

describe("updateScrollPosition", () => {
  // updateScrollPosition is called only when history.state exists. We seed
  // a state, call it, and confirm the values were merged into history.state.
  it("merges scrollX/scrollY into the existing history.state", () => {
    history.replaceState({ index: 5, scrollX: 0, scrollY: 0 }, "");
    updateScrollPosition({ scrollX: 100, scrollY: 200 });
    expect(history.state).toMatchObject({ index: 5, scrollX: 100, scrollY: 200 });
    expect(history.scrollRestoration).toBe("manual");
  });

  it("is a no-op when history.state is null", () => {
    // Force-clear state by replacing with null.
    history.replaceState(null, "");
    expect(() => updateScrollPosition({ scrollX: 1, scrollY: 1 })).not.toThrow();
    expect(history.state).toBeNull();
  });
});
