import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Island, resolveWhen } from "../island.js";
import { DEFAULT_WHEN, isWhen, WHEN_VALUES } from "../types.js";

describe("Island JSX wrapper", () => {
  it("renders a div with data-zfb-island marker and data-when='load' by default", () => {
    const node = Island({ children: "hello" });
    expect(node.type).toBe("div");
    expect(node.props["data-zfb-island"]).toBe("");
    expect(node.props["data-when"]).toBe("load");
    expect(node.props.children).toBe("hello");
  });

  it("forwards a valid `when` value to data-when", () => {
    for (const value of WHEN_VALUES) {
      const node = Island({ when: value, children: null });
      expect(node.props["data-when"]).toBe(value);
    }
  });

  it("does not set data-zfb-island to a component name (Sub 3 fills that in)", () => {
    const node = Island({ when: "visible", children: null });
    // The marker exists but holds an empty value at the build-time wrapper
    // stage. The hydration emit step (Sub 3) replaces it with the
    // component-name string when it walks rendered HTML.
    expect(node.props["data-zfb-island"]).toBe("");
  });

  it("preserves children verbatim, including arrays", () => {
    const children = ["a", "b", { type: "span", props: {}, key: null }];
    const node = Island({ when: "idle", children });
    expect(node.props.children).toBe(children);
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
