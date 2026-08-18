// Integration tests for <ClientRouter /> meta-tag emission.
//
// Verifies that ClientRouter conditionally appends a
// `<meta name="zfb-prefetch-disabled" content="true">` VNode to its output
// when `globalThis.__zfb.prefetchDisabled === true`, and does NOT emit the
// tag when the flag is absent or false.
//
// This pins the meta-tag contract with sibling sub-issue #276.

import { afterEach, describe, expect, it, vi } from "vitest";

// Imports the pure component module directly (#2437) — it has no
// module-scope side effects, so no vi.mock is needed to suppress `init()`
// (unlike the activation shim `../client-router.js`, which runs `init()` on
// import).
import { ClientRouter } from "../client-router-component.js";

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

  it("the zfb-prefetch-disabled meta node is a real JSX-runtime element (not a hand-rolled literal)", () => {
    (globalThis as { __zfb?: { prefetchDisabled?: boolean } }).__zfb = { prefetchDisabled: true };
    const nodes = ClientRouter();
    const prefetchMeta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-prefetch-disabled",
    );
    // Head nodes are minted via `jsx` from the per-project JSX runtime
    // (react in this test's config; preact in a Preact project via the engine
    // alias), NOT a hand-rolled `{ type, props, key, constructor: undefined }`
    // literal. A real element carries the `$$typeof` element brand
    // (`Symbol.for("react.element" | "react.transitional.element")`), which a
    // plain literal cannot have. This is what makes the node render under React
    // without error #31. The structural `.type`/`.props` surface is unchanged.
    expect(prefetchMeta).toBeDefined();
    const brand = (prefetchMeta as { $$typeof?: unknown } | undefined)?.$$typeof;
    expect(typeof brand).toBe("symbol");
    expect(prefetchMeta?.type).toBe("meta");
    expect(prefetchMeta?.props["content"]).toBe("true");
  });
});

describe("ClientRouter — preserveHtmlAttrs", () => {
  it("does not emit a zfb-preserve-html-attrs meta when the prop is omitted", () => {
    const nodes = ClientRouter();
    const meta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-preserve-html-attrs",
    );
    expect(meta).toBeUndefined();
    // Byte-identical baseline: still exactly the three base nodes.
    expect(nodes).toHaveLength(3);
  });

  it("does not emit the meta for an empty array", () => {
    const nodes = ClientRouter({ preserveHtmlAttrs: [] });
    const meta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-preserve-html-attrs",
    );
    expect(meta).toBeUndefined();
    expect(nodes).toHaveLength(3);
  });

  it("emits a space-joined zfb-preserve-html-attrs meta for a non-empty list", () => {
    const nodes = ClientRouter({ preserveHtmlAttrs: ["data-theme", "data-sidebar-hidden"] });
    const meta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-preserve-html-attrs",
    );
    expect(meta).toBeDefined();
    expect(meta?.props["content"]).toBe("data-theme data-sidebar-hidden");
    expect(nodes).toHaveLength(4);
  });

  it("filters falsy/empty entries and omits the meta when nothing remains", () => {
    const nodes = ClientRouter({ preserveHtmlAttrs: ["", ""] });
    const meta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-preserve-html-attrs",
    );
    expect(meta).toBeUndefined();
  });

  it("the preserve-attrs meta is a real JSX-runtime element (carries the $$typeof brand)", () => {
    const nodes = ClientRouter({ preserveHtmlAttrs: ["data-theme"] });
    const meta = nodes.find(
      (n) => n.type === "meta" && n.props["name"] === "zfb-preserve-html-attrs",
    );
    expect(meta).toBeDefined();
    const brand = (meta as { $$typeof?: unknown } | undefined)?.$$typeof;
    expect(typeof brand).toBe("symbol");
  });
});

describe("ClientRouter — traverseRefetch", () => {
  it("does not emit a zfb-traverse-refetch meta when the prop is omitted", () => {
    const nodes = ClientRouter();
    const meta = nodes.find((n) => n.type === "meta" && n.props["name"] === "zfb-traverse-refetch");
    expect(meta).toBeUndefined();
    // Byte-identical baseline: still exactly the three base nodes.
    expect(nodes).toHaveLength(3);
  });

  it("does not emit the meta when traverseRefetch is false", () => {
    const nodes = ClientRouter({ traverseRefetch: false });
    const meta = nodes.find((n) => n.type === "meta" && n.props["name"] === "zfb-traverse-refetch");
    expect(meta).toBeUndefined();
    expect(nodes).toHaveLength(3);
  });

  it("appends a zfb-traverse-refetch meta VNode when traverseRefetch is true", () => {
    const nodes = ClientRouter({ traverseRefetch: true });
    const meta = nodes.find((n) => n.type === "meta" && n.props["name"] === "zfb-traverse-refetch");
    expect(meta).toBeDefined();
    // Pin the exact attribute values — this is the contract the router reads.
    expect(meta?.props["name"]).toBe("zfb-traverse-refetch");
    expect(meta?.props["content"]).toBe("true");
    expect(nodes).toHaveLength(4);
  });

  it("the traverse-refetch meta is a real JSX-runtime element (carries the $$typeof brand)", () => {
    const nodes = ClientRouter({ traverseRefetch: true });
    const meta = nodes.find((n) => n.type === "meta" && n.props["name"] === "zfb-traverse-refetch");
    expect(meta).toBeDefined();
    const brand = (meta as { $$typeof?: unknown } | undefined)?.$$typeof;
    expect(typeof brand).toBe("symbol");
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
