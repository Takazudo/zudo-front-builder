// Integration tests for <ClientRouter /> meta-tag emission.
//
// Verifies that ClientRouter conditionally appends a
// `<meta name="zfb-prefetch-disabled" content="true">` VNode to its output
// when `globalThis.__zfb.prefetchDisabled === true`, and does NOT emit the
// tag when the flag is absent or false.
//
// This pins the meta-tag contract with sibling sub-issue #276.

import { afterEach, describe, expect, it, vi } from "vitest";

// vi.mock is hoisted to module top by Vitest, which lets us prevent the
// client-router module's side effect (`init()`) from running during
// the module-level `if (typeof document !== "undefined")` guard.
// We mock `./client-router/router.js` so `init` is a no-op spy.
vi.mock("../client-router/router.js", () => ({ init: vi.fn() }));

import { ClientRouter } from "../client-router.js";

afterEach(() => {
  // Clean up any globalThis.__zfb mutations between tests.
  delete (globalThis as { __zfb?: unknown }).__zfb;
  vi.unstubAllGlobals();
});

describe("ClientRouter — baseline (no prefetch flag)", () => {
  it("returns exactly three VNodes when __zfb.prefetchDisabled is absent", () => {
    const nodes = ClientRouter();
    expect(nodes).toHaveLength(3);
  });

  it("does not include a zfb-prefetch-disabled meta when __zfb is undefined", () => {
    delete (globalThis as { __zfb?: unknown }).__zfb;
    const nodes = ClientRouter();
    const prefetchMeta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-prefetch-disabled",
    );
    expect(prefetchMeta).toBeUndefined();
  });

  it("does not include a zfb-prefetch-disabled meta when __zfb.prefetchDisabled is false", () => {
    (globalThis as { __zfb?: { prefetchDisabled?: boolean } }).__zfb = { prefetchDisabled: false };
    const nodes = ClientRouter();
    const prefetchMeta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-prefetch-disabled",
    );
    expect(prefetchMeta).toBeUndefined();
  });
});

describe("ClientRouter — prefetch disabled flag", () => {
  it("appends a zfb-prefetch-disabled meta VNode when __zfb.prefetchDisabled === true", () => {
    (globalThis as { __zfb?: { prefetchDisabled?: boolean } }).__zfb = { prefetchDisabled: true };
    const nodes = ClientRouter();
    const prefetchMeta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-prefetch-disabled",
    );
    expect(prefetchMeta).toBeDefined();
    // Pin the exact attribute values — this is the verbatim contract with #276.
    expect(prefetchMeta?.props["name"]).toBe("zfb-prefetch-disabled");
    expect(prefetchMeta?.props["content"]).toBe("true");
  });

  it("returns four VNodes total when the flag is set", () => {
    (globalThis as { __zfb?: { prefetchDisabled?: boolean } }).__zfb = { prefetchDisabled: true };
    const nodes = ClientRouter();
    expect(nodes).toHaveLength(4);
  });

  it("the zfb-prefetch-disabled meta VNode has constructor === undefined (VNode shape)", () => {
    (globalThis as { __zfb?: { prefetchDisabled?: boolean } }).__zfb = { prefetchDisabled: true };
    const nodes = ClientRouter();
    const prefetchMeta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-prefetch-disabled",
    );
    // constructor: undefined is the sentinel for Preact/React structural VNodes.
    expect((prefetchMeta as { constructor?: unknown } | undefined)?.constructor).toBeUndefined();
  });
});

describe("ClientRouter — baseline nodes are always present", () => {
  it("always emits a style VNode first", () => {
    const nodes = ClientRouter();
    expect(nodes[0]?.type).toBe("style");
  });

  it("always emits the zfb-view-transitions-enabled meta", () => {
    const nodes = ClientRouter();
    const enabled = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-view-transitions-enabled",
    );
    expect(enabled?.props["content"]).toBe("true");
  });

  it("always emits the zfb-view-transitions-fallback meta with default animate", () => {
    const nodes = ClientRouter();
    const fallback = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-view-transitions-fallback",
    );
    expect(fallback?.props["content"]).toBe("animate");
  });
});
