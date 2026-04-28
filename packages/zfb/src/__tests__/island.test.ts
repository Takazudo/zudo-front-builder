import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  ANONYMOUS_COMPONENT_NAME,
  HYDRATE_MARKER_ATTR,
  Island,
  SKIP_SSR_MARKER_ATTR,
  captureComponentName,
  resolveWhen,
} from "../island.js";
import { DEFAULT_WHEN, isWhen, WHEN_VALUES } from "../types.js";

/**
 * Build a minimal, JSX-runtime-agnostic VNode.
 *
 * The Island wrapper inspects `child.type` to recover a component name; we
 * mirror the shape both Preact and React produce so the test does not
 * depend on either framework being installed.
 */
function vnode(type: unknown, props: Record<string, unknown> = {}) {
  return { type, props, key: null };
}

function NamedFn() {
  return null;
}

describe("Island JSX wrapper — default (hydrate) mode", () => {
  it("emits data-zfb-island=ComponentName from a function child", () => {
    const node = Island({ children: vnode(NamedFn) });
    expect(node.type).toBe("div");
    expect(node.props[HYDRATE_MARKER_ATTR]).toBe("NamedFn");
    expect(node.props["data-when"]).toBe("load");
    // SSR-skip marker must not be present.
    expect(node.props[SKIP_SSR_MARKER_ATTR]).toBeUndefined();
  });

  it("prefers child.type.displayName over child.type.name", () => {
    function Inner() {
      return null;
    }
    (Inner as unknown as { displayName: string }).displayName = "PrettyName";
    const node = Island({ children: vnode(Inner) });
    expect(node.props[HYDRATE_MARKER_ATTR]).toBe("PrettyName");
  });

  it("forwards a valid `when` value to data-when", () => {
    for (const value of WHEN_VALUES) {
      const node = Island({ when: value, children: vnode(NamedFn) });
      expect(node.props["data-when"]).toBe(value);
    }
  });

  it("preserves children verbatim", () => {
    const child = vnode(NamedFn);
    const node = Island({ when: "idle", children: child });
    expect(node.props.children).toBe(child);
  });

  it("uses Anonymous fallback for non-element children (e.g. plain strings)", () => {
    const node = Island({ children: "hello" });
    expect(node.props[HYDRATE_MARKER_ATTR]).toBe(ANONYMOUS_COMPONENT_NAME);
    expect(node.props.children).toBe("hello");
  });

  it("uses the first identifiable child when given an array", () => {
    const child1 = "raw text"; // not identifiable
    const child2 = vnode(NamedFn); // identifiable
    const node = Island({ children: [child1, child2] });
    expect(node.props[HYDRATE_MARKER_ATTR]).toBe("NamedFn");
  });

  it("uses host-tag string for string-typed children", () => {
    const node = Island({ children: vnode("section") });
    expect(node.props[HYDRATE_MARKER_ATTR]).toBe("section");
  });
});

describe("Island JSX wrapper — SSR-skip mode (client:only equivalent)", () => {
  it("switches markers when ssrFallback is provided", () => {
    const fallback = vnode("div", { children: "Loading…" });
    const node = Island({
      ssrFallback: fallback,
      children: vnode(NamedFn),
    });
    expect(node.type).toBe("div");
    // The hydrate marker must NOT appear; the SSR-skip marker MUST appear
    // and carry the captured component name.
    expect(node.props[HYDRATE_MARKER_ATTR]).toBeUndefined();
    expect(node.props[SKIP_SSR_MARKER_ATTR]).toBe("NamedFn");
    expect(node.props["data-when"]).toBe("load");
  });

  it("renders ssrFallback in place of children at SSR time", () => {
    const fallback = vnode("div", { children: "Loading…" });
    const heavy = vnode(NamedFn);
    const node = Island({ ssrFallback: fallback, children: heavy });
    // Children of the marker div must be the fallback, never the heavy
    // component — that's the whole point of SSR-skip mode.
    expect(node.props.children).toBe(fallback);
    expect(node.props.children).not.toBe(heavy);
  });

  it("renders null inside the marker when ssrFallback is null", () => {
    // Explicit null fallback is allowed; only `undefined` falls back to
    // the hydrate-mode emit.
    const node = Island({
      ssrFallback: null,
      children: vnode(NamedFn),
    });
    expect(node.props[SKIP_SSR_MARKER_ATTR]).toBe("NamedFn");
    expect(node.props.children).toBeNull();
  });

  it("still captures the component name even though the heavy child is not rendered", () => {
    function HeavyClientOnly() {
      // If this got called server-side the test would catch it because
      // the function would mutate the canary. The wrapper must NOT
      // invoke the child function under SSR-skip mode.
      throw new Error("HeavyClientOnly must not be invoked server-side");
    }
    const fallback = vnode("div", { children: "Loading…" });
    const node = Island({
      ssrFallback: fallback,
      children: vnode(HeavyClientOnly),
    });
    expect(node.props[SKIP_SSR_MARKER_ATTR]).toBe("HeavyClientOnly");
    // And the fallback content is what got rendered.
    expect(node.props.children).toBe(fallback);
  });
});

describe("captureComponentName", () => {
  it("returns Anonymous for non-element values", () => {
    expect(captureComponentName(undefined)).toBe(ANONYMOUS_COMPONENT_NAME);
    expect(captureComponentName(null)).toBe(ANONYMOUS_COMPONENT_NAME);
    expect(captureComponentName(0)).toBe(ANONYMOUS_COMPONENT_NAME);
    expect(captureComponentName("text")).toBe(ANONYMOUS_COMPONENT_NAME);
    expect(captureComponentName(true)).toBe(ANONYMOUS_COMPONENT_NAME);
  });

  it("returns the function's name for a function-typed child", () => {
    expect(captureComponentName(vnode(NamedFn))).toBe("NamedFn");
  });

  it("returns the host tag for a string-typed child", () => {
    expect(captureComponentName(vnode("section"))).toBe("section");
  });

  it("falls back to Anonymous for arrow functions with no name and no displayName", () => {
    const anon = (() => null) as unknown;
    Object.defineProperty(anon, "name", { value: "" });
    expect(captureComponentName(vnode(anon))).toBe(ANONYMOUS_COMPONENT_NAME);
  });
});

describe("resolveWhen", () => {
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    warnSpy.mockRestore();
  });

  it("returns the default when input is undefined", () => {
    expect(resolveWhen(undefined)).toBe(DEFAULT_WHEN);
    expect(warnSpy).not.toHaveBeenCalled();
  });

  it("returns the input unchanged when valid", () => {
    expect(resolveWhen("visible")).toBe("visible");
    expect(resolveWhen("idle")).toBe("idle");
    expect(resolveWhen("load")).toBe("load");
    expect(warnSpy).not.toHaveBeenCalled();
  });

  it("warns and falls back to load for unknown strings (in development)", () => {
    const original = process.env["NODE_ENV"];
    process.env["NODE_ENV"] = "development";
    try {
      expect(resolveWhen("eager")).toBe(DEFAULT_WHEN);
      expect(warnSpy).toHaveBeenCalledTimes(1);
      const msg = String(warnSpy.mock.calls[0]?.[0] ?? "");
      expect(msg).toContain("eager");
      expect(msg).toContain("load");
    } finally {
      if (original === undefined) {
        delete process.env["NODE_ENV"];
      } else {
        process.env["NODE_ENV"] = original;
      }
    }
  });

  it("does not warn in production builds", () => {
    const original = process.env["NODE_ENV"];
    process.env["NODE_ENV"] = "production";
    try {
      expect(resolveWhen("eager")).toBe(DEFAULT_WHEN);
      expect(warnSpy).not.toHaveBeenCalled();
    } finally {
      if (original === undefined) {
        delete process.env["NODE_ENV"];
      } else {
        process.env["NODE_ENV"] = original;
      }
    }
  });
});

describe("isWhen", () => {
  it("accepts the three valid strings only", () => {
    expect(isWhen("visible")).toBe(true);
    expect(isWhen("idle")).toBe(true);
    expect(isWhen("load")).toBe(true);
    expect(isWhen("eager")).toBe(false);
    expect(isWhen("")).toBe(false);
    expect(isWhen(undefined)).toBe(false);
    expect(isWhen(null)).toBe(false);
    expect(isWhen(0)).toBe(false);
  });
});
