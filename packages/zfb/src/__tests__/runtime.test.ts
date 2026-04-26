import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";

import { scheduleHydrate } from "../runtime.js";

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
