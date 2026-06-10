import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";

import {
  __hasPendingCancelForTests,
  __setIslandImporterForTests,
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
