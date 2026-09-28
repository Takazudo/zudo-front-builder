import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";

import {
  __hasPendingCancelForTests,
  ISLAND_MOUNTED_ATTR,
  mountIslands,
  mountNewIslands,
  scheduleHydrate,
  unmountIslands,
} from "../runtime.js";

type IntersectionCallback = (
  entries: Array<{ isIntersecting: boolean; target: Element }>,
  observer: { disconnect(): void; observe(el: Element): void },
) => void;

interface FakeObserverInstance {
  disconnect: ReturnType<typeof vi.fn>;
  observe: ReturnType<typeof vi.fn>;
  unobserve: ReturnType<typeof vi.fn>;
  trigger: (target: Element, isIntersecting: boolean) => void;
  options: { threshold?: number | number[] } | undefined;
}

describe("scheduleHydrate", () => {
  let target: HTMLElement;

  beforeEach(() => {
    target = document.createElement("div");
    document.body.appendChild(target);
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  describe("when='load'", () => {
    it("fires synchronously and returns a no-op cancel", () => {
      const fire = vi.fn();
      const cancel = scheduleHydrate(target, "load", fire);
      expect(fire).toHaveBeenCalledTimes(1);
      // Calling cancel after fire is safe and a no-op.
      expect(() => cancel()).not.toThrow();
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("treats omitted `when` as load (default)", () => {
      const fire = vi.fn();
      scheduleHydrate(target, undefined, fire);
      expect(fire).toHaveBeenCalledTimes(1);
    });
  });

  describe("when='idle'", () => {
    it("uses requestIdleCallback when available", () => {
      const calls: Array<() => void> = [];
      const ric = vi.fn((cb: () => void) => {
        calls.push(cb);
        return 42;
      });
      const cic = vi.fn();
      vi.stubGlobal("requestIdleCallback", ric);
      vi.stubGlobal("cancelIdleCallback", cic);

      const fire = vi.fn();
      scheduleHydrate(target, "idle", fire);

      expect(ric).toHaveBeenCalledTimes(1);
      expect(fire).not.toHaveBeenCalled();

      // Drain the queued idle callback.
      calls[0]?.();
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("falls back to setTimeout(0) when requestIdleCallback is absent", async () => {
      // Make sure happy-dom does not expose it.
      vi.stubGlobal("requestIdleCallback", undefined);

      vi.useFakeTimers();
      try {
        const fire = vi.fn();
        scheduleHydrate(target, "idle", fire);
        expect(fire).not.toHaveBeenCalled();
        vi.advanceTimersByTime(0);
        expect(fire).toHaveBeenCalledTimes(1);
      } finally {
        vi.useRealTimers();
      }
    });

    it("cancel() prevents the idle callback from firing", () => {
      const calls: Array<() => void> = [];
      const ric = vi.fn((cb: () => void) => {
        calls.push(cb);
        return 7;
      });
      const cic = vi.fn();
      vi.stubGlobal("requestIdleCallback", ric);
      vi.stubGlobal("cancelIdleCallback", cic);

      const fire = vi.fn();
      const cancel = scheduleHydrate(target, "idle", fire);
      cancel();
      // Even if the runtime delivers the callback, `fire` is gated by the
      // cancellation flag.
      calls[0]?.();
      expect(fire).not.toHaveBeenCalled();
      expect(cic).toHaveBeenCalledWith(7);
    });
  });

  describe("when='visible'", () => {
    let observers: FakeObserverInstance[];
    let ObserverSpy: MockInstance<
      (cb: IntersectionCallback, options?: { threshold?: number | number[] }) => unknown
    >;

    beforeEach(() => {
      observers = [];
      function FakeObserver(
        cb: IntersectionCallback,
        options?: { threshold?: number | number[] },
      ): FakeObserverInstance {
        const instance: FakeObserverInstance = {
          options,
          disconnect: vi.fn(),
          observe: vi.fn(),
          unobserve: vi.fn(),
          trigger(t: Element, isIntersecting: boolean) {
            cb([{ isIntersecting, target: t }], {
              disconnect: instance.disconnect,
              observe: instance.observe,
            });
          },
        };
        observers.push(instance);
        return instance;
      }
      ObserverSpy = vi.fn(FakeObserver) as unknown as typeof ObserverSpy;
      vi.stubGlobal("IntersectionObserver", ObserverSpy);
    });

    it("does not hydrate before intersection, hydrates on first intersection, then disconnects", () => {
      const fire = vi.fn();
      scheduleHydrate(target, "visible", fire);

      expect(ObserverSpy).toHaveBeenCalledTimes(1);
      const inst = observers[0];
      if (!inst) throw new Error("expected observer instance");
      expect(inst.observe).toHaveBeenCalledWith(target);
      expect(inst.options?.threshold).toBe(0);
      expect(fire).not.toHaveBeenCalled();

      // Non-intersecting entry is ignored.
      inst.trigger(target, false);
      expect(fire).not.toHaveBeenCalled();
      expect(inst.disconnect).not.toHaveBeenCalled();

      // First intersecting entry fires and disconnects.
      inst.trigger(target, true);
      expect(fire).toHaveBeenCalledTimes(1);
      expect(inst.disconnect).toHaveBeenCalledTimes(1);

      // Subsequent intersections do not re-fire.
      inst.trigger(target, true);
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("cancel() disconnects the observer and prevents firing", () => {
      const fire = vi.fn();
      const cancel = scheduleHydrate(target, "visible", fire);
      const inst = observers[0];
      if (!inst) throw new Error("expected observer instance");
      cancel();
      expect(inst.disconnect).toHaveBeenCalledTimes(1);
      // Even if a stale entry is delivered, fire is gated.
      inst.trigger(target, true);
      expect(fire).not.toHaveBeenCalled();
    });

    it("falls back to immediate fire when IntersectionObserver is missing", () => {
      vi.stubGlobal("IntersectionObserver", undefined);
      const fire = vi.fn();
      scheduleHydrate(target, "visible", fire);
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("public scheduleHydrate returns a plain function (not an object)", () => {
      // Verify the public API contract: scheduleHydrate must keep returning
      // a bare () => void cancel, regardless of changes to internal shape.
      vi.stubGlobal("IntersectionObserver", undefined);
      const cancel = scheduleHydrate(target, "visible", vi.fn());
      expect(typeof cancel).toBe("function");
    });
  });

  // ---------------------------------------------------------------------------
  // when='media' — matchMedia-gated hydration
  // ---------------------------------------------------------------------------

  describe("when='media'", () => {
    /** Build a fake MediaQueryList. `matches` controls the initial state. */
    function fakeMql(matches: boolean) {
      const listeners: Array<(e: MediaQueryListEvent) => void> = [];
      const mql = {
        matches,
        addEventListener: vi.fn((_type: string, handler: (e: MediaQueryListEvent) => void) => {
          listeners.push(handler);
        }),
        removeEventListener: vi.fn((_type: string, handler: (e: MediaQueryListEvent) => void) => {
          const idx = listeners.indexOf(handler);
          if (idx !== -1) listeners.splice(idx, 1);
        }),
        /** Simulate a media-query change event. */
        dispatchChange(newMatches: boolean) {
          const evt = { matches: newMatches } as MediaQueryListEvent;
          for (const l of [...listeners]) l(evt);
        },
        listenerCount() {
          return listeners.length;
        },
      };
      return mql;
    }

    type FakeMql = ReturnType<typeof fakeMql>;

    function stubMatchMedia(mql: FakeMql) {
      vi.stubGlobal(
        "matchMedia",
        vi.fn((_q: string) => mql),
      );
    }

    beforeEach(() => {
      target.setAttribute("data-media", "(max-width: 768px)");
    });

    it("fires synchronously when the query already matches + no pendingCancel registered", () => {
      const mql = fakeMql(true);
      stubMatchMedia(mql);
      // Ensure target is in the manifest as "Media" island for mountIslands test below.
      target.setAttribute("data-when", "media");
      target.setAttribute("data-media", "(max-width: 768px)");

      const fire = vi.fn();
      const cancel = scheduleHydrate(target, "media", fire);
      expect(fire).toHaveBeenCalledTimes(1);
      // No listener should have been registered — fired synchronously.
      expect(mql.addEventListener).not.toHaveBeenCalled();
      expect(typeof cancel).toBe("function");
    });

    it("does not fire when query does not initially match", () => {
      const mql = fakeMql(false);
      stubMatchMedia(mql);

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);
      expect(fire).not.toHaveBeenCalled();
      expect(mql.addEventListener).toHaveBeenCalledTimes(1);
    });

    it("fires exactly once on the first matching change event, then removes listener", () => {
      const mql = fakeMql(false);
      stubMatchMedia(mql);

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);
      expect(fire).not.toHaveBeenCalled();

      // Matching change event fires and removes listener.
      mql.dispatchChange(true);
      expect(fire).toHaveBeenCalledTimes(1);
      expect(mql.removeEventListener).toHaveBeenCalledTimes(1);

      // Second matching event is ignored (listener already removed).
      mql.dispatchChange(true);
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("ignores un-match (false) change events", () => {
      const mql = fakeMql(false);
      stubMatchMedia(mql);

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);

      // Un-match change — must NOT fire.
      mql.dispatchChange(false);
      expect(fire).not.toHaveBeenCalled();
      // Listener must still be registered (not consumed by the un-match).
      expect(mql.listenerCount()).toBe(1);
    });

    it("cancel() removes the matchMedia listener and prevents later firing", () => {
      const mql = fakeMql(false);
      stubMatchMedia(mql);

      const fire = vi.fn();
      const cancel = scheduleHydrate(target, "media", fire);
      expect(mql.addEventListener).toHaveBeenCalledTimes(1);

      cancel();
      expect(mql.removeEventListener).toHaveBeenCalledTimes(1);

      // Simulate the change event after cancel — fire must NOT be called.
      mql.dispatchChange(true);
      expect(fire).not.toHaveBeenCalled();
    });

    it("fails open (fires immediately) when matchMedia is absent", () => {
      vi.stubGlobal("matchMedia", undefined);

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("fails open (fires immediately) when data-media attribute is missing/empty", () => {
      const mql = fakeMql(false);
      stubMatchMedia(mql);
      // Remove the attribute so the scheduler can't find a query.
      target.removeAttribute("data-media");

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("falls back to legacy addListener/removeListener when addEventListener is missing (old Safari)", () => {
      // Older Safari (<14) MediaQueryList has no EventTarget API — only the
      // deprecated addListener/removeListener pair. The scheduler must not
      // throw and must still fire-once + clean up via removeListener.
      const listeners: Array<(e: MediaQueryListEvent) => void> = [];
      const legacyMql = {
        matches: false,
        addListener: vi.fn((h: (e: MediaQueryListEvent) => void) => {
          listeners.push(h);
        }),
        removeListener: vi.fn((h: (e: MediaQueryListEvent) => void) => {
          const i = listeners.indexOf(h);
          if (i !== -1) listeners.splice(i, 1);
        }),
        dispatchChange(m: boolean) {
          for (const l of [...listeners]) l({ matches: m } as MediaQueryListEvent);
        },
      };
      vi.stubGlobal(
        "matchMedia",
        vi.fn(() => legacyMql),
      );

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);
      expect(legacyMql.addListener).toHaveBeenCalledTimes(1);
      expect(fire).not.toHaveBeenCalled();

      legacyMql.dispatchChange(true);
      expect(fire).toHaveBeenCalledTimes(1);
      expect(legacyMql.removeListener).toHaveBeenCalledTimes(1);
    });

    it("legacy API: cancel() removes the listener via removeListener", () => {
      const listeners: Array<(e: MediaQueryListEvent) => void> = [];
      const legacyMql = {
        matches: false,
        addListener: vi.fn((h: (e: MediaQueryListEvent) => void) => {
          listeners.push(h);
        }),
        removeListener: vi.fn((h: (e: MediaQueryListEvent) => void) => {
          const i = listeners.indexOf(h);
          if (i !== -1) listeners.splice(i, 1);
        }),
        dispatchChange(m: boolean) {
          for (const l of [...listeners]) l({ matches: m } as MediaQueryListEvent);
        },
      };
      vi.stubGlobal(
        "matchMedia",
        vi.fn(() => legacyMql),
      );

      const fire = vi.fn();
      const cancel = scheduleHydrate(target, "media", fire);
      cancel();
      expect(legacyMql.removeListener).toHaveBeenCalledTimes(1);
      legacyMql.dispatchChange(true);
      expect(fire).not.toHaveBeenCalled();
    });

    it("fails open when MediaQueryList exposes no listener API at all", () => {
      vi.stubGlobal(
        "matchMedia",
        vi.fn(() => ({ matches: false })),
      );

      const fire = vi.fn();
      scheduleHydrate(target, "media", fire);
      expect(fire).toHaveBeenCalledTimes(1);
    });

    it("mountIslands-level: data-when=media island hydrates on first match", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-when="media" data-media="(max-width: 768px)"></div>
      `;
      const mql = fakeMql(false);
      vi.stubGlobal(
        "matchMedia",
        vi.fn(() => mql),
      );

      const mount = vi.fn();

      mountIslands({ Counter: { mount } });

      expect(mount).not.toHaveBeenCalled();
      mql.dispatchChange(true);
      expect(mount).toHaveBeenCalledTimes(1);
      expect(mount.mock.calls[0]![2]).toBe("hydrate");
    });

    it("lazy props: data-props is NOT parsed until the media query matches", () => {
      // Set up an island with malformed data-props — if props are parsed eagerly
      // at boot time, this test would surface the malformed parse there. With
      // lazy parse, the malformed attribute should not be touched until fire.
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-when="media" data-media="(max-width: 768px)" data-props='NOT_JSON'></div>
      `;
      const mql = fakeMql(false);
      vi.stubGlobal(
        "matchMedia",
        vi.fn(() => mql),
      );

      const mount = vi.fn();

      // Boot time schedules the listener without parsing malformed props.
      mountIslands({ Counter: { mount } });
      expect(mount).not.toHaveBeenCalled();
    });
  });

  describe("mountIslands", () => {
    it("hydrates SSR islands with parsed data-props and the right mode", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":3}' data-when="load">
          <button>3</button>
        </div>
      `;

      const mount = vi.fn();

      mountIslands({ Counter: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
      const args = mount.mock.calls[0]!;
      expect(args[0]).toEqual({ start: 3 });
      // Element is the data-zfb-island wrapper.
      expect((args[1] as Element).getAttribute("data-zfb-island")).toBe("Counter");
      // SSR'd islands hydrate.
      expect(args[2]).toBe("hydrate");
    });

    it("renders SSR-skip islands (mode=render) immediately, ignoring data-when", () => {
      document.body.innerHTML = `
        <div data-zfb-island-skip-ssr="Modal" data-props='{"open":true}' data-when="visible"></div>
      `;

      const mount = vi.fn();

      mountIslands({ Modal: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
      // SSR-skip mounts via render, not hydrate, so React/Preact won't
      // emit hydration-mismatch warnings against an empty container.
      expect(mount.mock.calls[0]![2]).toBe("render");
    });

    it("does not mount the same element twice across repeat calls", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();

      mountIslands({ Counter: { mount } });
      mountIslands({ Counter: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
    });

    it("falls back to {} props when data-props is missing or invalid", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-when="load"></div>
        <div data-zfb-island-skip-ssr="Modal" data-props="not json"></div>
      `;
      const mount = vi.fn();

      mountIslands({
        Counter: { mount },
        Modal: { mount },
      });

      expect(mount).toHaveBeenCalledTimes(2);
      expect(mount.mock.calls[0]![0]).toEqual({});
      expect(mount.mock.calls[1]![0]).toEqual({});
    });

    it("falls back to {} props when data-props is a JSON array (not a record)", () => {
      // `typeof [] === "object"` so the old guard let arrays through.
      // Arrays are not a valid props bag — we must reject them and
      // hand the component an empty record instead.
      const arrayProps = JSON.stringify([1, 2, 3]);
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='${arrayProps}' data-when="load"></div>
      `;
      const mount = vi.fn();

      mountIslands({ Counter: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
      expect(mount.mock.calls[0]![0]).toEqual({});
    });

    it("warns and skips elements whose component is missing from the manifest", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Mystery" data-props='{}' data-when="load"></div>
      `;
      const original = process.env["NODE_ENV"];
      process.env["NODE_ENV"] = "development";
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
      const mount = vi.fn();

      try {
        mountIslands({ Counter: { mount } });
        expect(mount).not.toHaveBeenCalled();
        expect(warnSpy).toHaveBeenCalledTimes(1);
      } finally {
        warnSpy.mockRestore();
        if (original === undefined) {
          delete process.env["NODE_ENV"];
        } else {
          process.env["NODE_ENV"] = original;
        }
      }
    });

    it("uses the module's default export when mount is not present", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const def = vi.fn();

      mountIslands({ Counter: { default: def } });
      expect(def).toHaveBeenCalledTimes(1);
    });

    // ---------------------------------------------------------------------
    // The shared bundle supplies inline IslandModule descriptors.
    // ---------------------------------------------------------------------
    it("calls inline-module mount synchronously (SSR path)", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":7}' data-when="load"></div>
      `;
      const mount = vi.fn();

      mountIslands({ Counter: { mount } });

      // Synchronous mount: no microtask flush required.
      expect(mount).toHaveBeenCalledTimes(1);
      const args = mount.mock.calls[0]!;
      expect(args[0]).toEqual({ start: 7 });
      expect((args[1] as Element).getAttribute("data-zfb-island")).toBe("Counter");
      expect(args[2]).toBe("hydrate");
    });

    it("calls inline-module mount with mode=render for SSR-skip islands", () => {
      document.body.innerHTML = `
        <div data-zfb-island-skip-ssr="Modal" data-props='{"open":true}' data-when="visible"></div>
      `;
      const mount = vi.fn();

      mountIslands({ Modal: { mount } });

      // SSR-skip ignores data-when and mounts immediately.
      expect(mount).toHaveBeenCalledTimes(1);
      expect(mount.mock.calls[0]![2]).toBe("render");
    });

    it("falls back to default export on inline-module entry when mount is absent", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const def = vi.fn();

      mountIslands({ Counter: { default: def } });

      expect(def).toHaveBeenCalledTimes(1);
    });

    it("does not double-mount inline-module entries on repeat calls", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();

      mountIslands({ Counter: { mount } });
      mountIslands({ Counter: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
    });

    // -----------------------------------------------------------------------
    // unmountIslands() test cases (#274)
    // -----------------------------------------------------------------------

    it("unmountIslands(root) calls the module's unmount with the correct element", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":1}' data-when="load"></div>
      `;
      const mount = vi.fn();
      const unmount = vi.fn();

      mountIslands({ Counter: { mount, unmount } });

      expect(mount).toHaveBeenCalledTimes(1);

      // Calling unmountIslands with a root that contains the mounted island
      // should invoke the bundle's unmount function with the element.
      const el = document.querySelector("[data-zfb-island]")!;
      unmountIslands(document.body);

      expect(unmount).toHaveBeenCalledTimes(1);
      expect(unmount).toHaveBeenCalledWith(el);
    });

    it("unmountIslands does not throw when module exposes no unmount", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();
      // Module has no unmount export — the runtime stores a noop thunk.

      mountIslands({ Counter: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
      // Must not throw even though no unmount was exposed by the bundle.
      expect(() => unmountIslands(document.body)).not.toThrow();
    });

    it("unmountIslands honours unmount from shared-bundle inline manifest entry", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":5}' data-when="load"></div>
      `;
      const mount = vi.fn();
      const unmount = vi.fn();

      // Inline IslandModule shape (shared-bundle path) with unmount.
      mountIslands({ Counter: { mount, unmount } });

      // Inline mount is synchronous.
      expect(mount).toHaveBeenCalledTimes(1);

      const el = document.querySelector("[data-zfb-island]")!;
      unmountIslands(document.body);

      expect(unmount).toHaveBeenCalledTimes(1);
      expect(unmount).toHaveBeenCalledWith(el);
    });

    // -----------------------------------------------------------------------
    // data-zfb-transition-persist lifecycle across a body swap (#1389).
    //
    // The client-router hands unmountIslands() the INCOMING body so it can skip
    // islands swapBodyElement will lift (a persist id present on both sides).
    // These use the inline-module manifest shape so mount/unmount are
    // synchronous and directly observable. swapBodyElement lives in the sibling
    // @takazudo/zfb-runtime package; here we simulate only the DOM effects it
    // produces (the data-zfb-island-remount flag + refreshed data-props). The real
    // swapBodyElement is exercised end-to-end in zfb-runtime's
    // persist-island-lifecycle.test.ts.
    // -----------------------------------------------------------------------
    describe("persist lifecycle across a body swap (#1389)", () => {
      const PERSIST = "data-zfb-transition-persist";
      const incomingBody = (inner: string): HTMLElement =>
        new DOMParser().parseFromString(
          `<!doctype html><html><body>${inner}</body></html>`,
          "text/html",
        ).body;

      it("unmountIslands SKIPS a persisted island whose id matches the incoming body", () => {
        document.body.innerHTML = `
          <div ${PERSIST}="chrome" data-zfb-island="Sidebar" data-props='{"open":true}' data-when="load"></div>
          <div data-zfb-island="Toc" data-props='{"page":1}' data-when="load"></div>
        `;
        const sidebarUnmount = vi.fn();
        const tocUnmount = vi.fn();
        mountIslands({
          Sidebar: { mount: vi.fn(), unmount: sidebarUnmount },
          Toc: { mount: vi.fn(), unmount: tocUnmount },
        });

        unmountIslands(
          document.body,
          incomingBody(`
            <div ${PERSIST}="chrome" data-zfb-island="Sidebar" data-props='{"open":true}' data-when="load"></div>
            <div data-zfb-island="Toc" data-props='{"page":2}' data-when="load"></div>
          `),
        );

        // Persisted island survives the lift — its framework unmount must NOT fire.
        expect(sidebarUnmount).not.toHaveBeenCalled();
        // Non-persisted island still unmounts as before.
        expect(tocUnmount).toHaveBeenCalledTimes(1);
      });

      it("the persisted island's mounted entry survives, so mountNewIslands does NOT re-mount it (but DOES re-mount the discarded one)", () => {
        document.body.innerHTML = `
          <div ${PERSIST}="chrome" data-zfb-island="Sidebar" data-props='{"open":true}' data-when="load"></div>
          <div data-zfb-island="Toc" data-props='{"page":1}' data-when="load"></div>
        `;
        const sidebarMount = vi.fn();
        const tocMount = vi.fn();
        mountIslands({
          Sidebar: { mount: sidebarMount, unmount: vi.fn() },
          Toc: { mount: tocMount, unmount: vi.fn() },
        });
        expect(sidebarMount).toHaveBeenCalledTimes(1);
        expect(tocMount).toHaveBeenCalledTimes(1);

        unmountIslands(
          document.body,
          incomingBody(`
            <div ${PERSIST}="chrome" data-zfb-island="Sidebar" data-props='{"open":true}' data-when="load"></div>
            <div data-zfb-island="Toc" data-props='{"page":2}' data-when="load"></div>
          `),
        );
        // Re-walk the (still-live) body: persisted stays mounted, discarded remounts.
        mountNewIslands();

        // Sidebar was never re-mounted → its mounted-map entry (and instance) survived.
        expect(sidebarMount).toHaveBeenCalledTimes(1);
        // Toc was unmounted → mountNewIslands mounts a fresh instance.
        expect(tocMount).toHaveBeenCalledTimes(2);
      });

      it("unmountIslands STILL unmounts a persisted island when the incoming body lacks the id", () => {
        document.body.innerHTML = `
          <div ${PERSIST}="gone" data-zfb-island="Orphan" data-props='{}' data-when="load"></div>
        `;
        const unmount = vi.fn();
        mountIslands({ Orphan: { mount: vi.fn(), unmount } });

        // Incoming body has no matching persist id → swapBodyElement would discard
        // it → it must be unmounted here.
        unmountIslands(document.body, incomingBody(`<p>fresh</p>`));

        expect(unmount).toHaveBeenCalledTimes(1);
      });

      it("unmountIslands with no incoming body unmounts a persisted island (pre-#1389 back-compat)", () => {
        document.body.innerHTML = `
          <div ${PERSIST}="chrome" data-zfb-island="Sidebar" data-props='{}' data-when="load"></div>
        `;
        const unmount = vi.fn();
        mountIslands({ Sidebar: { mount: vi.fn(), unmount } });

        // Single-arg call (no swap in flight) preserves nothing — identical to
        // the original walk.
        unmountIslands(document.body);

        expect(unmount).toHaveBeenCalledTimes(1);
      });

      it("mountNewIslands consumes the remount flag: unmounts the stale instance and re-mounts with fresh props (remount queue)", () => {
        document.body.innerHTML = `
          <div ${PERSIST}="panel" data-zfb-island="Panel" data-props='{"v":1}' data-when="load"></div>
        `;
        const el = document.querySelector(`[${PERSIST}="panel"]`)!;
        const mount = vi.fn();
        const unmount = vi.fn();
        mountIslands({ Panel: { mount, unmount } });
        expect(mount).toHaveBeenCalledTimes(1);
        expect(mount.mock.calls[0]![0]).toEqual({ v: 1 });

        // Simulate swapBodyElement's persist-props branch: the surviving element
        // gets the refreshed props + the data-zfb-island-remount flag.
        el.setAttribute("data-props", '{"v":2}');
        el.setAttribute("data-zfb-island-remount", "");

        mountNewIslands();

        // The stale instance was torn down exactly once, then a fresh one mounted
        // with the new props.
        expect(unmount).toHaveBeenCalledTimes(1);
        expect(mount).toHaveBeenCalledTimes(2);
        expect(mount.mock.calls[1]![0]).toEqual({ v: 2 });
        // The flag is consumed so the remount happens exactly once.
        expect(el.hasAttribute("data-zfb-island-remount")).toBe(false);
      });

      it("mounted inline deferred remount bypasses the scheduler and runs synchronously", () => {
        vi.useFakeTimers();
        try {
          vi.stubGlobal("requestIdleCallback", undefined);
          document.body.innerHTML = `
            <div ${PERSIST}="panel" data-zfb-island="Panel" data-props='{"v":1}' data-when="idle"></div>
          `;
          const el = document.querySelector(`[${PERSIST}="panel"]`)!;
          const mount = vi.fn();
          const unmount = vi.fn();

          mountIslands({ Panel: { mount, unmount } });
          expect(mount).not.toHaveBeenCalled();

          vi.advanceTimersByTime(0);
          expect(mount).toHaveBeenCalledTimes(1);
          expect(mount.mock.calls[0]![0]).toEqual({ v: 1 });

          el.setAttribute("data-props", '{"v":2}');
          el.setAttribute("data-zfb-island-remount", "");

          mountNewIslands();

          // No second timer advance: a persisted props-change remount must not
          // wait for idle again after the old instance has been unmounted.
          expect(unmount).toHaveBeenCalledTimes(1);
          expect(mount).toHaveBeenCalledTimes(2);
          expect(mount.mock.calls[1]![0]).toEqual({ v: 2 });
          expect(el.hasAttribute("data-zfb-island-remount")).toBe(false);
        } finally {
          vi.useRealTimers();
        }
      });

      it("mountNewIslands leaves a persisted island with unchanged props (no remount flag) mounted — no unmount, no re-mount", () => {
        document.body.innerHTML = `
          <div ${PERSIST}="chrome" data-zfb-island="Sidebar" data-props='{"open":true}' data-when="load"></div>
        `;
        const mount = vi.fn();
        const unmount = vi.fn();
        mountIslands({ Sidebar: { mount, unmount } });
        expect(mount).toHaveBeenCalledTimes(1);

        // No remount flag (props were identical) → mountNewIslands must not disturb it.
        mountNewIslands();

        expect(unmount).not.toHaveBeenCalled();
        expect(mount).toHaveBeenCalledTimes(1);
      });
    });

    // -----------------------------------------------------------------------
    // Stale-pendingCancels bug (#743): when IntersectionObserver is absent,
    // scheduleVisible fires synchronously and returns noop — but the old code
    // always called pendingCancels.set(element, noop) when when !== "load",
    // leaving a permanent stale entry. The fix: only set pendingCancels when
    // the scheduler did NOT fire synchronously (!fired).
    // -----------------------------------------------------------------------

    it("fireInlineMount path: no stale pendingCancels entry when IO-less when=visible fires synchronously", () => {
      // Remove IntersectionObserver so scheduleVisible fails open (sync fire).
      vi.stubGlobal("IntersectionObserver", undefined);

      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="visible"></div>
      `;

      const mount = vi.fn();

      const el = document.querySelector("[data-zfb-island]")!;

      // Inline-module path (shared-bundle): mount is called directly.
      mountIslands({ Counter: { mount } });

      // The scheduler fired synchronously, so there must be NO stale entry
      // in pendingCancels for this element. (#743)
      expect(__hasPendingCancelForTests(el)).toBe(false);
    });

    // -----------------------------------------------------------------------
    // Nested-island self-wrap warnings (#859)
    // -----------------------------------------------------------------------

    describe("nested island self-wrap warnings", () => {
      it("(a) warns once when a data-zfb-island element is nested inside another island marker", () => {
        document.body.innerHTML = `
          <div data-zfb-island="Outer" data-props='{}' data-when="load">
            <div data-zfb-island="Inner" data-props='{}' data-when="load"></div>
          </div>
        `;
        const original = process.env["NODE_ENV"];
        process.env["NODE_ENV"] = "development";
        const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
          mountIslands({ Outer: { mount: vi.fn() }, Inner: { mount: vi.fn() } });
          // Only the nested "Inner" island should trigger the warning.
          expect(warnSpy).toHaveBeenCalledTimes(1);
          expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining("Inner"));
          expect(warnSpy.mock.calls[0]![0] as string).toContain("call site");
        } finally {
          warnSpy.mockRestore();
          if (original === undefined) {
            delete process.env["NODE_ENV"];
          } else {
            process.env["NODE_ENV"] = original;
          }
        }
      });

      it("(b) warns when a data-zfb-island-skip-ssr element is nested inside an island marker", () => {
        document.body.innerHTML = `
          <div data-zfb-island="Outer" data-props='{}' data-when="load">
            <div data-zfb-island-skip-ssr="InnerSkip" data-props='{}'></div>
          </div>
        `;
        const original = process.env["NODE_ENV"];
        process.env["NODE_ENV"] = "development";
        const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
          mountIslands({ Outer: { mount: vi.fn() }, InnerSkip: { mount: vi.fn() } });
          expect(warnSpy).toHaveBeenCalledTimes(1);
          expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining("InnerSkip"));
        } finally {
          warnSpy.mockRestore();
          if (original === undefined) {
            delete process.env["NODE_ENV"];
          } else {
            process.env["NODE_ENV"] = original;
          }
        }
      });

      it("(c) does NOT warn for a flat (non-nested) single island", () => {
        document.body.innerHTML = `
          <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
        `;
        const original = process.env["NODE_ENV"];
        process.env["NODE_ENV"] = "development";
        const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
          mountIslands({ Counter: { mount: vi.fn() } });
          expect(warnSpy).not.toHaveBeenCalled();
        } finally {
          warnSpy.mockRestore();
          if (original === undefined) {
            delete process.env["NODE_ENV"];
          } else {
            process.env["NODE_ENV"] = original;
          }
        }
      });

      it("(d) does NOT warn for sibling (non-nested) islands", () => {
        document.body.innerHTML = `
          <div data-zfb-island="Alpha" data-props='{}' data-when="load"></div>
          <div data-zfb-island="Beta" data-props='{}' data-when="load"></div>
        `;
        const original = process.env["NODE_ENV"];
        process.env["NODE_ENV"] = "development";
        const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
          mountIslands({ Alpha: { mount: vi.fn() }, Beta: { mount: vi.fn() } });
          expect(warnSpy).not.toHaveBeenCalled();
        } finally {
          warnSpy.mockRestore();
          if (original === undefined) {
            delete process.env["NODE_ENV"];
          } else {
            process.env["NODE_ENV"] = original;
          }
        }
      });

      it("warns at most once per nested element across repeated mountIslands / mountNewIslands calls", () => {
        document.body.innerHTML = `
          <div data-zfb-island="Outer" data-props='{}' data-when="load">
            <div data-zfb-island="Inner" data-props='{}' data-when="load"></div>
          </div>
        `;
        const original = process.env["NODE_ENV"];
        process.env["NODE_ENV"] = "development";
        const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
          mountIslands({ Outer: { mount: vi.fn() }, Inner: { mount: vi.fn() } });
          // Second walk (e.g. SPA swap re-check) should not re-warn.
          mountNewIslands();
          expect(warnSpy).toHaveBeenCalledTimes(1);
        } finally {
          warnSpy.mockRestore();
          if (original === undefined) {
            delete process.env["NODE_ENV"];
          } else {
            process.env["NODE_ENV"] = original;
          }
        }
      });

      it("does NOT warn in production (NODE_ENV=production)", () => {
        document.body.innerHTML = `
          <div data-zfb-island="Outer" data-props='{}' data-when="load">
            <div data-zfb-island="Inner" data-props='{}' data-when="load"></div>
          </div>
        `;
        const original = process.env["NODE_ENV"];
        process.env["NODE_ENV"] = "production";
        const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
          mountIslands({ Outer: { mount: vi.fn() }, Inner: { mount: vi.fn() } });
          expect(warnSpy).not.toHaveBeenCalled();
        } finally {
          warnSpy.mockRestore();
          if (original === undefined) {
            delete process.env["NODE_ENV"];
          } else {
            process.env["NODE_ENV"] = original;
          }
        }
      });
    });
  });

  describe("unknown when=", () => {
    it("warns and falls back to load (immediate)", () => {
      const original = process.env["NODE_ENV"];
      process.env["NODE_ENV"] = "development";
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
      try {
        const fire = vi.fn();
        scheduleHydrate(target, "eager", fire);
        expect(fire).toHaveBeenCalledTimes(1);
        expect(warnSpy).toHaveBeenCalledTimes(1);
      } finally {
        warnSpy.mockRestore();
        if (original === undefined) {
          delete process.env["NODE_ENV"];
        } else {
          process.env["NODE_ENV"] = original;
        }
      }
    });
  });
});

describe("island mounted marker state contract (#2541)", () => {
  function island(selector = "[data-zfb-island]"): HTMLElement {
    const el = document.querySelector<HTMLElement>(selector);
    if (!el) throw new Error(`expected island ${selector}`);
    return el;
  }

  function isMounted(el: Element): boolean {
    return el.hasAttribute(ISLAND_MOUNTED_ATTR);
  }

  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("initially has no mounted marker", () => {
    document.body.innerHTML = `<div data-zfb-island="Probe" data-when="load"></div>`;

    expect(isMounted(island())).toBe(false);
  });

  it("source issue repro: mounts Probe with when=load and writes the marker", () => {
    document.body.innerHTML = `<div data-zfb-island="Probe" data-when="load"></div>`;
    const mount = vi.fn();

    mountIslands({ Probe: { mount } });

    const probe = island('[data-zfb-island="Probe"]');
    expect(mount).toHaveBeenCalledTimes(1);
    expect(probe.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
  });

  it("keeps the marker absent while an idle island is deferred", () => {
    vi.useFakeTimers();
    vi.stubGlobal("requestIdleCallback", undefined);
    document.body.innerHTML = `
      <div data-zfb-island="Idle" data-when="idle"></div>
    `;
    const el = island();
    const mount = vi.fn();

    mountIslands({ Idle: { mount } });

    expect(isMounted(el)).toBe(false);
    expect(mount).not.toHaveBeenCalled();

    vi.advanceTimersByTime(0);

    expect(mount).toHaveBeenCalledTimes(1);
    expect(isMounted(el)).toBe(true);
  });

  it("keeps the marker absent until a visible island intersects", () => {
    document.body.innerHTML = `
      <div data-zfb-island="Visible" data-when="visible"></div>
    `;
    const el = island();
    const mount = vi.fn();
    const observer = { disconnect: vi.fn(), observe: vi.fn() };
    let trigger: ((isIntersecting: boolean) => void) | undefined;
    const Observer = vi.fn((callback: IntersectionCallback) => {
      trigger = (isIntersecting) => {
        callback([{ isIntersecting, target: el }], observer);
      };
      return observer;
    });
    vi.stubGlobal("IntersectionObserver", Observer);

    mountIslands({ Visible: { mount } });

    expect(isMounted(el)).toBe(false);
    expect(mount).not.toHaveBeenCalled();

    trigger!(false);
    expect(isMounted(el)).toBe(false);
    trigger!(true);

    expect(mount).toHaveBeenCalledTimes(1);
    expect(isMounted(el)).toBe(true);
  });

  it("keeps the marker absent until a media island first matches", () => {
    document.body.innerHTML = `
      <div
        data-zfb-island="Media"
        data-when="media"
        data-media="(max-width: 768px)"
      ></div>
    `;
    const el = island();
    const listeners: Array<(event: MediaQueryListEvent) => void> = [];
    const mql = {
      matches: false,
      addEventListener: vi.fn((_type: string, listener: (event: MediaQueryListEvent) => void) => {
        listeners.push(listener);
      }),
      removeEventListener: vi.fn(
        (_type: string, listener: (event: MediaQueryListEvent) => void) => {
          const index = listeners.indexOf(listener);
          if (index !== -1) listeners.splice(index, 1);
        },
      ),
    };
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => mql),
    );
    const mount = vi.fn();

    mountIslands({ Media: { mount } });

    expect(isMounted(el)).toBe(false);
    expect(mount).not.toHaveBeenCalled();

    listeners[0]?.({ matches: false } as MediaQueryListEvent);
    expect(isMounted(el)).toBe(false);
    listeners[0]?.({ matches: true } as MediaQueryListEvent);

    expect(mount).toHaveBeenCalledTimes(1);
    expect(isMounted(el)).toBe(true);
  });

  it("writes the marker for an inline SSR-skip island after mount returns", () => {
    document.body.innerHTML = `
      <div data-zfb-island-skip-ssr="Modal" data-when="visible"></div>
    `;
    const el = island('[data-zfb-island-skip-ssr="Modal"]');
    const mount = vi.fn(() => {
      expect(isMounted(el)).toBe(false);
    });

    mountIslands({ Modal: { mount } });

    expect(mount).toHaveBeenCalledTimes(1);
    expect(mount.mock.calls[0]![2]).toBe("render");
    expect(el.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
  });

  it("leaves the marker absent for a missing manifest entry", () => {
    document.body.innerHTML = `<div data-zfb-island="Missing" data-when="load"></div>`;
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    mountIslands({});

    expect(isMounted(island())).toBe(false);
    expect(warnSpy).toHaveBeenCalled();
  });

  it("leaves the marker absent when an inline module has no mount export", () => {
    document.body.innerHTML = `<div data-zfb-island="NoMount" data-when="load"></div>`;
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    mountIslands({ NoMount: {} });

    expect(isMounted(island())).toBe(false);
    expect(warnSpy).toHaveBeenCalled();
  });

  it("clears the marker after an inline mount throws and allows a successful retry", () => {
    document.body.innerHTML = `<div data-zfb-island="Throws" data-when="load"></div>`;
    const el = island();
    let shouldThrow = true;
    const mount = vi.fn(() => {
      expect(isMounted(el)).toBe(false);
      if (shouldThrow) throw new Error("inline mount failed");
    });

    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => mountIslands({ Throws: { mount } })).not.toThrow();
    expect(error).toHaveBeenCalledWith(
      expect.stringContaining('island "Throws" mount failed'),
      expect.any(Error),
    );
    expect(isMounted(el)).toBe(false);

    shouldThrow = false;
    mountIslands({ Throws: { mount } });

    expect(mount).toHaveBeenCalledTimes(2);
    expect(isMounted(el)).toBe(true);
  });

  it("clears the marker when an island is unmounted", () => {
    document.body.innerHTML = `<div data-zfb-island="Counter" data-when="load"></div>`;
    const el = island();
    const unmount = vi.fn();

    mountIslands({ Counter: { mount: vi.fn(), unmount } });
    expect(isMounted(el)).toBe(true);

    unmountIslands(document.body);

    expect(unmount).toHaveBeenCalledWith(el);
    expect(isMounted(el)).toBe(false);
  });

  it("clears the marker even when unmount throws", () => {
    document.body.innerHTML = `<div data-zfb-island="Counter" data-when="load"></div>`;
    const el = island();
    const unmount = vi.fn(() => {
      throw new Error("unmount failed");
    });
    mountIslands({ Counter: { mount: vi.fn(), unmount } });
    expect(isMounted(el)).toBe(true);

    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => unmountIslands(document.body)).not.toThrow();
    expect(error).toHaveBeenCalledWith(
      expect.stringContaining('island "Counter" disposal failed'),
      expect.any(Error),
    );
    expect(isMounted(el)).toBe(false);
  });

  it("strips a stale marker when a fresh runtime module hot-swaps over the DOM", async () => {
    document.body.innerHTML = `<div data-zfb-island="Probe" data-when="load"></div>`;
    const el = island();
    const previousMount = vi.fn();
    mountIslands({ Probe: { mount: previousMount } });
    expect(isMounted(el)).toBe(true);

    vi.resetModules();
    const freshRuntime = await import("../runtime.js");
    const freshMount = vi.fn(() => {
      // mountIslands() must strip the old module's marker before scheduling
      // this fresh module's mount.
      expect(isMounted(el)).toBe(false);
    });

    freshRuntime.mountIslands({ Probe: { mount: freshMount } });

    expect(freshMount).toHaveBeenCalledTimes(1);
    expect(isMounted(el)).toBe(true);
  });
});
