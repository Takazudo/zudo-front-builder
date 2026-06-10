import { afterEach, describe, expect, it, vi } from "vitest";
import { hydrateAll } from "../src/hydrate.ts";

// Each test reaches into the `happy-dom` document. Reset between tests.
afterEach(() => {
  document.body.innerHTML = "";
  document.head.innerHTML = "";
});

/** Build a fake islands bundle exposing two named components + adapter shim. */
function makeBundle() {
  const calls: Array<{
    name: string;
    Component: unknown;
    props: unknown;
    element: Element;
  }> = [];
  const bundle = {
    Counter: { __id: "counter" },
    Banner: { __id: "banner" },
    hydrateIsland(Component: unknown, props: unknown, element: Element) {
      const named = Object.entries(bundle).find(([, v]) => v === Component) ?? ["?", Component];
      calls.push({
        name: named[0],
        Component,
        props,
        element,
      });
    },
  };
  return { bundle, calls };
}

describe("hydrateAll — happy path", () => {
  it("registers a single load island and calls hydrateIsland once", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props='{"start":3}'>
        <button>3</button>
      </div>
    `;
    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/dist/assets/islands-x.js",
      importBundle: async () => bundle,
    });

    expect(calls).toHaveLength(1);
    expect(calls[0]!.name).toBe("Counter");
    expect(calls[0]!.props).toEqual({ start: 3 });
    expect(calls[0]!.element).toBeInstanceOf(Element);
  });

  it("treats absent data-when as load (immediate hydration)", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Banner" data-props="{}"></div>
    `;
    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    // No microtask gymnastics needed: load is synchronous.
    expect(calls).toHaveLength(1);
    expect(calls[0]!.name).toBe("Banner");
  });

  it("explicitly honours data-when=load the same as absent", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Banner" data-props="{}" data-when="load"></div>
    `;
    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    expect(calls).toHaveLength(1);
  });

  it("hydrates multiple islands in document order", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props='{"n":1}'></div>
      <div data-zfb-island="Banner" data-props='{"n":2}'></div>
    `;
    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    expect(calls.map((c) => c.name)).toEqual(["Counter", "Banner"]);
  });

  it("decodes HTML-escaped JSON in data-props (& and quote)", async () => {
    // The Rust rewriter encodes `"` as `&quot;` and `&` as `&amp;` so the
    // attribute survives serialization. Going through innerHTML (the
    // browser's parser) decodes those entities back to the original JSON
    // string. hydrateAll then JSON.parses the decoded value.
    document.body.innerHTML =
      `<div data-zfb-island="Counter" ` +
      `data-props="{&quot;label&quot;:&quot;A &amp; B&quot;}"></div>`;
    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    expect(calls[0]!.props).toEqual({ label: "A & B" });
  });
});

describe("hydrateAll — error paths", () => {
  it("throws when an island has no data-zfb-island name", async () => {
    document.body.innerHTML = `<div data-zfb-island="" data-props="{}"></div>`;
    const { bundle } = makeBundle();
    await expect(
      hydrateAll({
        bundleUrl: "/x.js",
        importBundle: async () => bundle,
      }),
    ).rejects.toThrow(/no data-zfb-island component name/);
  });

  it("throws when the named component is not exported by the bundle", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Mystery" data-props="{}"></div>
    `;
    const { bundle } = makeBundle();
    await expect(
      hydrateAll({
        bundleUrl: "/x.js",
        importBundle: async () => bundle,
      }),
    ).rejects.toThrow(/no export "Mystery"/);
  });

  it("throws when the bundle has no hydrateIsland export", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}"></div>
    `;
    await expect(
      hydrateAll({
        bundleUrl: "/x.js",
        importBundle: async () =>
          ({ Counter: {} }) as unknown as Parameters<typeof hydrateAll>[0] extends infer _
            ? never
            : never,
      }),
    ).rejects.toThrow(/no hydrateIsland export/);
  });

  it("throws when no [data-zfb-bundle] script and no override is supplied", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}"></div>
    `;
    await expect(hydrateAll()).rejects.toThrow(/no <script data-zfb-bundle>/);
  });

  it("does nothing when no islands are present (no bundle import)", async () => {
    const importBundle = vi.fn();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle,
    });
    expect(importBundle).not.toHaveBeenCalled();
  });
});

describe("hydrateAll — data-when=idle", () => {
  it("runs hydration via requestIdleCallback when available", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="idle"></div>
    `;
    const ric = vi.fn((cb: () => void) => {
      cb();
      return 1;
    });
    (
      globalThis as unknown as {
        requestIdleCallback: typeof ric;
      }
    ).requestIdleCallback = ric;

    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    expect(ric).toHaveBeenCalledTimes(1);
    expect(calls).toHaveLength(1);
    delete (
      globalThis as unknown as {
        requestIdleCallback?: unknown;
      }
    ).requestIdleCallback;
  });

  it("falls back to setTimeout when requestIdleCallback is missing", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="idle"></div>
    `;
    const stOriginal = globalThis.setTimeout;
    const st = vi.fn((cb: () => void) => {
      cb();
      return 0 as unknown as ReturnType<typeof setTimeout>;
    });
    (globalThis as unknown as { setTimeout: typeof st }).setTimeout = st;
    delete (globalThis as unknown as { requestIdleCallback?: unknown }).requestIdleCallback;
    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    expect(st).toHaveBeenCalledTimes(1);
    expect(calls).toHaveLength(1);
    (globalThis as unknown as { setTimeout: typeof stOriginal }).setTimeout = stOriginal;
  });
});

describe("hydrateAll — data-when=media", () => {
  /** Build a controllable fake MediaQueryList. */
  function fakeMql(matches: boolean) {
    const listeners: Array<(e: MediaQueryListEvent) => void> = [];
    return {
      matches,
      addEventListener: vi.fn((_type: string, h: (e: MediaQueryListEvent) => void) => {
        listeners.push(h);
      }),
      removeEventListener: vi.fn((_type: string, h: (e: MediaQueryListEvent) => void) => {
        const i = listeners.indexOf(h);
        if (i !== -1) listeners.splice(i, 1);
      }),
      dispatchChange(m: boolean) {
        for (const l of [...listeners]) l({ matches: m } as MediaQueryListEvent);
      },
    };
  }

  it("does not hydrate before the media query matches", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="media" data-media="(max-width: 768px)"></div>
    `;
    const mql = fakeMql(false);
    (globalThis as unknown as { matchMedia: unknown }).matchMedia = vi.fn(() => mql);

    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });

    expect(mql.addEventListener).toHaveBeenCalledTimes(1);
    expect(calls).toHaveLength(0);
    delete (globalThis as unknown as { matchMedia?: unknown }).matchMedia;
  });

  it("fires once when the query matches", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="media" data-media="(max-width: 768px)"></div>
    `;
    const mql = fakeMql(false);
    (globalThis as unknown as { matchMedia: unknown }).matchMedia = vi.fn(() => mql);

    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });

    expect(calls).toHaveLength(0);

    // Simulate query match.
    mql.dispatchChange(true);

    expect(calls).toHaveLength(1);
    expect(calls[0]!.name).toBe("Counter");

    // Listener removed after first match.
    expect(mql.removeEventListener).toHaveBeenCalledTimes(1);
    delete (globalThis as unknown as { matchMedia?: unknown }).matchMedia;
  });

  it("ignores un-match change events", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="media" data-media="(max-width: 768px)"></div>
    `;
    const mql = fakeMql(false);
    (globalThis as unknown as { matchMedia: unknown }).matchMedia = vi.fn(() => mql);

    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });

    mql.dispatchChange(false); // un-match — must be ignored
    expect(calls).toHaveLength(0);
    delete (globalThis as unknown as { matchMedia?: unknown }).matchMedia;
  });

  it("fires synchronously when the query already matches at boot (no listener registered)", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="media" data-media="(min-width: 0px)"></div>
    `;
    const mql = fakeMql(true); // already matches
    (globalThis as unknown as { matchMedia: unknown }).matchMedia = vi.fn(() => mql);

    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });

    expect(calls).toHaveLength(1);
    expect(mql.addEventListener).not.toHaveBeenCalled();
    delete (globalThis as unknown as { matchMedia?: unknown }).matchMedia;
  });

  it("npm lazy-props: data-props is NOT read until the query matches", async () => {
    // Use non-matching query so the fire closure is deferred.
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props='NOT_JSON' data-when="media" data-media="(max-width: 768px)"></div>
    `;
    const mql = fakeMql(false);
    (globalThis as unknown as { matchMedia: unknown }).matchMedia = vi.fn(() => mql);

    const { bundle } = makeBundle();
    // hydrateAll completes without parsing props (query not yet matched).
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });
    // No error thrown — props not yet parsed. Good.
    delete (globalThis as unknown as { matchMedia?: unknown }).matchMedia;
  });
});

describe("hydrateAll — data-when=visible", () => {
  it("waits for IntersectionObserver intersection before hydrating", async () => {
    document.body.innerHTML = `
      <div data-zfb-island="Counter" data-props="{}" data-when="visible"></div>
    `;
    let savedCallback: IntersectionObserverCallback | null = null;
    let savedTarget: Element | null = null;
    const observe = vi.fn((target: Element) => {
      savedTarget = target;
    });
    const unobserve = vi.fn();
    const disconnect = vi.fn();
    class FakeIO {
      constructor(cb: IntersectionObserverCallback) {
        savedCallback = cb;
      }
      observe = observe;
      unobserve = unobserve;
      disconnect = disconnect;
      takeRecords = () => [] as IntersectionObserverEntry[];
      readonly root = null;
      readonly rootMargin = "0px";
      readonly thresholds: ReadonlyArray<number> = [0];
    }
    (globalThis as unknown as { IntersectionObserver: typeof FakeIO }).IntersectionObserver =
      FakeIO;

    const { bundle, calls } = makeBundle();
    await hydrateAll({
      bundleUrl: "/x.js",
      importBundle: async () => bundle,
    });

    // Before intersection: observe was registered but hydrate has not run.
    expect(observe).toHaveBeenCalledTimes(1);
    expect(calls).toHaveLength(0);

    // Simulate the element scrolling into view.
    savedCallback!(
      [
        {
          isIntersecting: true,
          target: savedTarget!,
        } as IntersectionObserverEntry,
      ],
      // The runtime calls obs.unobserve / obs.disconnect on the second
      // argument; pass an object that exposes them.
      { unobserve, disconnect } as unknown as IntersectionObserver,
    );

    // Implementation detail: scheduleHydrate from "zfb/runtime" calls
    // disconnect() on the observer (which also stops further callbacks);
    // it does not call unobserve() separately. The outcome is the same:
    // hydration runs exactly once for this island.
    expect(disconnect).toHaveBeenCalledTimes(1);
    expect(calls).toHaveLength(1);
  });
});
