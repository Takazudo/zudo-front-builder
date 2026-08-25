import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";

import {
  __hasPendingCancelForTests,
  __setIslandImporterForTests,
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

    it("mountIslands-level: data-when=media island hydrates on first match", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-when="media" data-media="(max-width: 768px)"></div>
      `;
      const mql = fakeMql(false);
      vi.stubGlobal(
        "matchMedia",
        vi.fn(() => mql),
      );

      const mount = vi.fn();
      const restoreImporter = __setIslandImporterForTests(async () => ({ mount }));
      try {
        mountIslands({ Counter: "/islands/Counter-abc.js" });

        // Before the query matches: import must not have started.
        expect(mount).not.toHaveBeenCalled();

        // Simulate query match.
        mql.dispatchChange(true);
        await Promise.resolve();
        await Promise.resolve();

        expect(mount).toHaveBeenCalledTimes(1);
        expect(mount.mock.calls[0]![2]).toBe("hydrate");
      } finally {
        __setIslandImporterForTests(restoreImporter);
      }
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
      const restoreImporter = __setIslandImporterForTests(async () => ({ mount }));
      try {
        // Boot time: scheduleMount should set up the listener but NOT parse props.
        mountIslands({ Counter: { mount } });

        // The island should not have mounted yet (query doesn't match).
        expect(mount).not.toHaveBeenCalled();
        // Props are not parsed until fire — no crash here.
      } finally {
        __setIslandImporterForTests(restoreImporter);
      }
    });
  });

  describe("mountIslands", () => {
    type FakeMount = (
      props: Record<string, unknown>,
      element: Element,
      mode: "hydrate" | "render",
    ) => void;
    type FakeModule = { mount?: FakeMount; default?: FakeMount };
    let restoreImporter: (url: string) => Promise<FakeModule>;

    beforeEach(() => {
      // Default: a fresh load=immediate scheduler with no IO observers.
      // Individual tests stub the importer below.
    });

    afterEach(() => {
      if (typeof restoreImporter === "function") {
        __setIslandImporterForTests(restoreImporter);
      }
    });

    it("hydrates SSR islands with parsed data-props and the right mode", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":3}' data-when="load">
          <button>3</button>
        </div>
      `;

      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async (_url) => ({
        mount,
      }));

      mountIslands({ Counter: "/islands/Counter-abc.js" });
      // Allow microtasks queued by the dynamic import promise.
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(1);
      const args = mount.mock.calls[0]!;
      expect(args[0]).toEqual({ start: 3 });
      // Element is the data-zfb-island wrapper.
      expect((args[1] as Element).getAttribute("data-zfb-island")).toBe("Counter");
      // SSR'd islands hydrate.
      expect(args[2]).toBe("hydrate");
    });

    it("renders SSR-skip islands (mode=render) immediately, ignoring data-when", async () => {
      document.body.innerHTML = `
        <div data-zfb-island-skip-ssr="Modal" data-props='{"open":true}' data-when="visible"></div>
      `;

      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ mount }));

      mountIslands({ Modal: "/islands/Modal-def.js" });
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(1);
      // SSR-skip mounts via render, not hydrate, so React/Preact won't
      // emit hydration-mismatch warnings against an empty container.
      expect(mount.mock.calls[0]![2]).toBe("render");
    });

    it("does not mount the same element twice across repeat calls", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ mount }));

      mountIslands({ Counter: "/islands/Counter-abc.js" });
      await Promise.resolve();
      await Promise.resolve();
      mountIslands({ Counter: "/islands/Counter-abc.js" });
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(1);
    });

    it("does not double-mount when concurrent invocations race the dynamic import", async () => {
      // Round 2 regression: before the `pending` guard, two concurrent
      // `mountIslands` calls would both pass the `mounted` check
      // (synchronous) and both spawn `importIsland(url) -> fn(...)`.
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();

      // A deferred importer: its Promise only resolves once we explicitly
      // flush. That gives us a window during which BOTH `mountIslands`
      // calls are simultaneously waiting on the import.
      let resolveImport: ((mod: { mount: typeof mount }) => void) | undefined;
      const importPromise = new Promise<{ mount: typeof mount }>((resolve) => {
        resolveImport = resolve;
      });
      restoreImporter = __setIslandImporterForTests(() => importPromise);

      mountIslands({ Counter: "/islands/Counter-abc.js" });
      mountIslands({ Counter: "/islands/Counter-abc.js" });

      // Now resolve the import — only ONE `mount(...)` call should fire.
      resolveImport!({ mount });
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(1);
    });

    it("falls back to {} props when data-props is missing or invalid", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-when="load"></div>
        <div data-zfb-island-skip-ssr="Modal" data-props="not json"></div>
      `;
      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ mount }));

      mountIslands({
        Counter: "/islands/Counter-abc.js",
        Modal: "/islands/Modal-def.js",
      });
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(2);
      expect(mount.mock.calls[0]![0]).toEqual({});
      expect(mount.mock.calls[1]![0]).toEqual({});
    });

    it("falls back to {} props when data-props is a JSON array (not a record)", async () => {
      // `typeof [] === "object"` so the old guard let arrays through.
      // Arrays are not a valid props bag — we must reject them and
      // hand the component an empty record instead.
      const arrayProps = JSON.stringify([1, 2, 3]);
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='${arrayProps}' data-when="load"></div>
      `;
      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ mount }));

      mountIslands({ Counter: "/islands/Counter-abc.js" });
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(1);
      expect(mount.mock.calls[0]![0]).toEqual({});
    });

    it("warns and skips elements whose component is missing from the manifest", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Mystery" data-props='{}' data-when="load"></div>
      `;
      const original = process.env["NODE_ENV"];
      process.env["NODE_ENV"] = "development";
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ mount }));
      try {
        mountIslands({ Counter: "/islands/Counter-abc.js" });
        await Promise.resolve();
        await Promise.resolve();
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

    it("uses the bundle's default export when mount is not present", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const def = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ default: def }));
      mountIslands({ Counter: "/islands/Counter-abc.js" });
      await Promise.resolve();
      await Promise.resolve();
      expect(def).toHaveBeenCalledTimes(1);
    });

    // ---------------------------------------------------------------------
    // Inline-module manifest shape (issue #146 / zudolab/zudo-doc#1355
    // wave 6). The shared-bundle production path imports every island's
    // source code into one bundle and hands `mountIslands` an object
    // whose values are `IslandModule` descriptors instead of URL
    // strings. The runtime must call those mount functions directly,
    // skipping the dynamic import entirely.
    // ---------------------------------------------------------------------
    it("calls inline-module mount synchronously without dynamic import (SSR path)", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":7}' data-when="load"></div>
      `;
      const mount = vi.fn();
      // The importer must NOT be invoked when the manifest value is an
      // inline module — the test fails the importer to make that loud.
      restoreImporter = __setIslandImporterForTests(() => {
        throw new Error("dynamic import must not be used for inline-module manifest entries");
      });

      mountIslands({ Counter: { mount } });

      // Synchronous: no microtask flush required because we never went
      // through `import()` for this entry.
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
      restoreImporter = __setIslandImporterForTests(() => {
        throw new Error("dynamic import must not be used for inline-module manifest entries");
      });

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
      restoreImporter = __setIslandImporterForTests(() => {
        throw new Error("dynamic import must not be used for inline-module manifest entries");
      });

      mountIslands({ Counter: { default: def } });

      expect(def).toHaveBeenCalledTimes(1);
    });

    it("does not double-mount inline-module entries on repeat calls", () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(() => {
        throw new Error("dynamic import must not be used for inline-module manifest entries");
      });

      mountIslands({ Counter: { mount } });
      mountIslands({ Counter: { mount } });

      expect(mount).toHaveBeenCalledTimes(1);
    });

    // -----------------------------------------------------------------------
    // unmountIslands() test cases (#274)
    // -----------------------------------------------------------------------

    it("unmountIslands(root) calls the bundle's unmount with the correct element", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{"start":1}' data-when="load"></div>
      `;
      const mount = vi.fn();
      const unmount = vi.fn();
      restoreImporter = __setIslandImporterForTests(async () => ({ mount, unmount }));

      mountIslands({ Counter: "/islands/Counter-abc.js" });
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).toHaveBeenCalledTimes(1);

      // Calling unmountIslands with a root that contains the mounted island
      // should invoke the bundle's unmount function with the element.
      const el = document.querySelector("[data-zfb-island]")!;
      unmountIslands(document.body);

      expect(unmount).toHaveBeenCalledTimes(1);
      expect(unmount).toHaveBeenCalledWith(el);
    });

    it("unmountIslands does not throw when bundle exposes no unmount", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();
      // Bundle has no unmount export — the runtime stores a noop thunk.
      restoreImporter = __setIslandImporterForTests(async () => ({ mount }));

      mountIslands({ Counter: "/islands/Counter-abc.js" });
      await Promise.resolve();
      await Promise.resolve();

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
      restoreImporter = __setIslandImporterForTests(() => {
        throw new Error("dynamic import must not be used for inline-module manifest entries");
      });

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

      it("pending URL remount keeps the flag until import resolves and mounts with refreshed props", async () => {
        document.body.innerHTML = `
          <div ${PERSIST}="panel" data-zfb-island="Panel" data-props='{"v":1}' data-when="load"></div>
        `;
        const el = document.querySelector(`[${PERSIST}="panel"]`)!;
        const mount = vi.fn();

        let resolveImport: ((mod: { mount: typeof mount }) => void) | undefined;
        const importPromise = new Promise<{ mount: typeof mount }>((resolve) => {
          resolveImport = resolve;
        });
        restoreImporter = __setIslandImporterForTests(() => importPromise);

        mountIslands({ Panel: "/islands/panel.js" });
        expect(mount).not.toHaveBeenCalled();

        // Simulate swapBodyElement refreshing props while the URL import is still pending.
        el.setAttribute("data-props", '{"v":2}');
        el.setAttribute("data-zfb-island-remount", "");

        mountNewIslands();

        // The pending import owns the eventual mount, so the flag must not be
        // consumed before that import can re-read fresh data-props.
        expect(el.hasAttribute("data-zfb-island-remount")).toBe(true);
        expect(mount).not.toHaveBeenCalled();

        resolveImport!({ mount });
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();

        expect(mount).toHaveBeenCalledTimes(1);
        expect(mount.mock.calls[0]![0]).toEqual({ v: 2 });
        expect(el.hasAttribute("data-zfb-island-remount")).toBe(false);

        mountNewIslands();
        await Promise.resolve();
        await Promise.resolve();

        expect(mount).toHaveBeenCalledTimes(1);
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

      it("mounted URL deferred remount starts immediately instead of waiting for idle again", async () => {
        vi.useFakeTimers();
        try {
          vi.stubGlobal("requestIdleCallback", undefined);
          document.body.innerHTML = `
            <div ${PERSIST}="panel" data-zfb-island="Panel" data-props='{"v":1}' data-when="idle"></div>
          `;
          const el = document.querySelector(`[${PERSIST}="panel"]`)!;
          const mount = vi.fn();
          const unmount = vi.fn();
          const importer = vi.fn(async () => ({ mount, unmount }));
          restoreImporter = __setIslandImporterForTests(importer);

          mountIslands({ Panel: "/islands/panel.js" });
          expect(importer).not.toHaveBeenCalled();

          vi.advanceTimersByTime(0);
          await Promise.resolve();
          await Promise.resolve();

          expect(importer).toHaveBeenCalledTimes(1);
          expect(mount).toHaveBeenCalledTimes(1);
          expect(mount.mock.calls[0]![0]).toEqual({ v: 1 });

          el.setAttribute("data-props", '{"v":2}');
          el.setAttribute("data-zfb-island-remount", "");

          mountNewIslands();

          // No second timer advance: the replacement URL import should start
          // immediately even though the actual mount remains promise-timed.
          expect(unmount).toHaveBeenCalledTimes(1);
          expect(importer).toHaveBeenCalledTimes(2);

          await Promise.resolve();
          await Promise.resolve();

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

    it("stale-mount race: does not call mount when element is detached before import resolves", async () => {
      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="load"></div>
      `;
      const mount = vi.fn();

      // Deferred importer: only resolves after we explicitly flush.
      let resolveImport: ((mod: { mount: typeof mount }) => void) | undefined;
      const importPromise = new Promise<{ mount: typeof mount }>((resolve) => {
        resolveImport = resolve;
      });
      restoreImporter = __setIslandImporterForTests(() => importPromise);

      mountIslands({ Counter: "/islands/Counter-abc.js" });

      // Detach the element from the DOM to simulate a body swap while the
      // import is still in-flight.
      const el = document.querySelector("[data-zfb-island]")!;
      el.remove();
      expect(el.isConnected).toBe(false);

      // Now resolve the import — the isConnected guard must prevent mount()
      // from being called for the detached element.
      resolveImport!({ mount });
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();

      expect(mount).not.toHaveBeenCalled();
    });

    // -----------------------------------------------------------------------
    // Stale-pendingCancels bug (#743): when IntersectionObserver is absent,
    // scheduleVisible fires synchronously and returns noop — but the old code
    // always called pendingCancels.set(element, noop) when when !== "load",
    // leaving a permanent stale entry. The fix: only set pendingCancels when
    // the scheduler did NOT fire synchronously (!fired).
    // -----------------------------------------------------------------------

    it("URL path: no stale pendingCancels entry when IO-less when=visible fires synchronously", () => {
      // Remove IntersectionObserver so scheduleVisible fails open (sync fire).
      vi.stubGlobal("IntersectionObserver", undefined);

      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="visible"></div>
      `;

      // Use an importer that resolves synchronously (via a resolved Promise)
      // so console.error is not triggered. We just care about pendingCancels.
      restoreImporter = __setIslandImporterForTests(() => Promise.resolve({ mount: vi.fn() }));

      const el = document.querySelector("[data-zfb-island]")!;

      mountIslands({ Counter: "/islands/Counter-abc.js" });

      // The scheduler fired synchronously, so there must be NO stale entry
      // in pendingCancels for this element. (#743)
      expect(__hasPendingCancelForTests(el)).toBe(false);
    });

    it("fireInlineMount path: no stale pendingCancels entry when IO-less when=visible fires synchronously", () => {
      // Remove IntersectionObserver so scheduleVisible fails open (sync fire).
      vi.stubGlobal("IntersectionObserver", undefined);

      document.body.innerHTML = `
        <div data-zfb-island="Counter" data-props='{}' data-when="visible"></div>
      `;

      const mount = vi.fn();
      restoreImporter = __setIslandImporterForTests(() => {
        throw new Error("dynamic import must not be used for inline-module manifest entries");
      });

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
  type Mount = (
    props: Record<string, unknown>,
    element: Element,
    mode: "hydrate" | "render",
  ) => void;
  type Module = {
    mount?: Mount;
    default?: Mount;
    unmount?: (element: Element) => void;
  };
  type Importer = (url: string) => Promise<Module>;

  let restoreImporter: Importer | undefined;

  function stubImporter(importer: Importer): void {
    restoreImporter = __setIslandImporterForTests(importer);
  }

  async function flushImport(): Promise<void> {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  }

  // Deliberately controlled thenable: capturing the runtime's fulfillment
  // callback lets the test observe a synchronous mount throw directly and
  // retry without creating an unhandled child rejection.
  function controlledImport(module: Module): {
    promise: Promise<Module>;
    fulfill: () => unknown;
  } {
    let onFulfilled: ((value: Module) => unknown) | undefined;
    const promise = {
      then(fulfillment: (value: Module) => unknown): Promise<void> {
        onFulfilled = fulfillment;
        return Promise.resolve();
      },
    } as unknown as Promise<Module>;

    return {
      promise,
      fulfill: () => {
        if (!onFulfilled) throw new Error("import fulfillment callback was not registered");
        return onFulfilled(module);
      },
    };
  }

  function island(selector = "[data-zfb-island]"): HTMLElement {
    const el = document.querySelector<HTMLElement>(selector);
    if (!el) throw new Error(`expected island ${selector}`);
    return el;
  }

  function isMounted(el: Element): boolean {
    return el.hasAttribute(ISLAND_MOUNTED_ATTR);
  }

  afterEach(() => {
    if (restoreImporter) {
      __setIslandImporterForTests(restoreImporter);
      restoreImporter = undefined;
    }
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

  it("keeps the marker absent while an idle URL island is deferred", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("requestIdleCallback", undefined);
    document.body.innerHTML = `
      <div data-zfb-island="Idle" data-when="idle"></div>
    `;
    const el = island();
    const mount = vi.fn();
    stubImporter(async () => ({ mount }));

    mountIslands({ Idle: "/islands/Idle.js" });

    expect(isMounted(el)).toBe(false);
    expect(mount).not.toHaveBeenCalled();

    vi.advanceTimersByTime(0);
    await flushImport();

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

  it("writes the marker for a URL island only after mount returns", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-when="load"></div>
    `;
    const el = island();
    const mount = vi.fn(() => {
      expect(isMounted(el)).toBe(false);
    });
    let resolveImport: ((module: Module) => void) | undefined;
    stubImporter(
      () =>
        new Promise((resolve) => {
          resolveImport = resolve;
        }),
    );

    mountIslands({ Counter: "/islands/Counter.js" });
    expect(isMounted(el)).toBe(false);
    expect(mount).not.toHaveBeenCalled();

    resolveImport!({ mount });
    await flushImport();

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

  it("leaves the marker absent when a URL module has no mount export", async () => {
    document.body.innerHTML = `<div data-zfb-island="NoMount" data-when="load"></div>`;
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    stubImporter(async () => ({}));

    mountIslands({ NoMount: "/islands/NoMount.js" });
    await flushImport();

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

  it("clears the marker after a URL mount throws and allows a successful retry", async () => {
    document.body.innerHTML = `<div data-zfb-island="Throws" data-when="load"></div>`;
    const el = island();
    const throwingMount = vi.fn(() => {
      throw new Error("URL mount failed");
    });
    const successfulMount = vi.fn(() => {
      expect(isMounted(el)).toBe(false);
    });
    const firstImport = controlledImport({ mount: throwingMount });
    let importerCalls = 0;
    stubImporter(() => {
      importerCalls += 1;
      return importerCalls === 1
        ? firstImport.promise
        : Promise.resolve({ mount: successfulMount });
    });

    mountIslands({ Throws: "/islands/Throws.js" });
    expect(() => firstImport.fulfill()).toThrow("URL mount failed");

    expect(throwingMount).toHaveBeenCalledTimes(1);
    expect(isMounted(el)).toBe(false);

    mountIslands({ Throws: "/islands/Throws.js" });
    await flushImport();

    expect(importerCalls).toBe(2);
    expect(successfulMount).toHaveBeenCalledTimes(1);
    expect(isMounted(el)).toBe(true);
  });

  it("clears the marker after an inline mount throws and allows a successful retry", () => {
    document.body.innerHTML = `<div data-zfb-island="Throws" data-when="load"></div>`;
    const el = island();
    let shouldThrow = true;
    const mount = vi.fn(() => {
      expect(isMounted(el)).toBe(false);
      if (shouldThrow) throw new Error("inline mount failed");
    });

    expect(() => mountIslands({ Throws: { mount } })).toThrow("inline mount failed");
    expect(isMounted(el)).toBe(false);

    shouldThrow = false;
    mountIslands({ Throws: { mount } });

    expect(mount).toHaveBeenCalledTimes(2);
    expect(isMounted(el)).toBe(true);
  });

  it("leaves the marker absent when a URL import is rejected", async () => {
    document.body.innerHTML = `<div data-zfb-island="Rejected" data-when="load"></div>`;
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    stubImporter(async () => {
      throw new Error("import failed");
    });

    mountIslands({ Rejected: "/islands/Rejected.js" });
    await flushImport();

    expect(isMounted(island())).toBe(false);
    expect(errorSpy).toHaveBeenCalled();
  });

  it("leaves the marker absent when a URL island detaches during import", async () => {
    document.body.innerHTML = `<div data-zfb-island="Detached" data-when="load"></div>`;
    const el = island();
    const mount = vi.fn();
    let resolveImport: ((module: Module) => void) | undefined;
    stubImporter(
      () =>
        new Promise((resolve) => {
          resolveImport = resolve;
        }),
    );

    mountIslands({ Detached: "/islands/Detached.js" });
    el.remove();
    expect(isMounted(el)).toBe(false);

    resolveImport!({ mount });
    await flushImport();

    expect(mount).not.toHaveBeenCalled();
    expect(isMounted(el)).toBe(false);
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

    expect(() => unmountIslands(document.body)).toThrow("unmount failed");
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
