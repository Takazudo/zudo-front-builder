/**
 * Shared SSR spec for the render-region sentinel markers (epic #2421).
 *
 * `buildContentComponent` must emit the pinned `<template
 * data-zfb-render-region>` pair around every bridge-resolved content
 * region when the build-only `globalThis.__zfb.renderArtifacts` switch is
 * on, and must be byte-neutral when it is off — under BOTH JSX runtimes
 * the bridge supports. The wrapper builds its Fragment from
 * `react/jsx-runtime`, which the engine alias-rewrites to
 * `preact/jsx-runtime` in Preact mode, so "it works in React" is not
 * evidence that it works in Preact (or the reverse).
 *
 * The cases therefore live here once and are driven twice — see
 * `content-render-markers-react.test.ts` (real `react-dom/server`) and
 * `content-render-markers-preact.test.ts` (real `preact-render-to-string`,
 * with the same `react/jsx-runtime` → `preact/jsx-runtime` swap the
 * bundler performs). Assertions are on ACTUAL rendered bytes, never on
 * JSX structure: a stray whitespace or text node between the sentinels and
 * the region would be invisible to a structural check and fatal to the
 * build's exact-byte extraction pass.
 *
 * Not a `*.test.ts` file, so vitest's `include` glob never collects it on
 * its own.
 */

import { afterEach, describe, expect, it } from "vitest";
import { jsx, jsxs } from "react/jsx-runtime";

import { getCollection, setContentSnapshot } from "../content.js";
import type { CollectionEntry, ContentProps, Snapshot } from "../content.js";

/** Region ids are the entries' `module_specifier`s. */
const OUTER_ID = "mdx://blog/outer#0a1b2c3d";
const INNER_ID = "mdx://blog/inner#4e5f6a7b";

const SNAPSHOT: Snapshot = {
  collections: {
    blog: [
      {
        slug: "outer",
        frontmatter: { title: "Outer" },
        body: "# outer body",
        module_specifier: OUTER_ID,
        rel_path: "blog/outer.md",
      },
      {
        slug: "inner",
        frontmatter: { title: "Inner" },
        body: "# inner body",
        module_specifier: INNER_ID,
        rel_path: "blog/inner.md",
      },
    ],
  },
};

/**
 * The slots these cases drive on the shared `globalThis.__zfb` namespace.
 * Narrowed to what is touched here — `contentSnapshot` is installed through
 * `setContentSnapshot` rather than written directly.
 */
type BridgeGlobal = typeof globalThis & {
  __zfb?: {
    content?: { get(specifier: string): ((props: ContentProps) => unknown) | undefined };
    renderArtifacts?: boolean;
  };
};

/** Start/end sentinel bytes for one region id. */
function marker(edge: "start" | "end", regionId: string): string {
  return `<template data-zfb-render-region="${edge}" data-zfb-region-id="${regionId}"></template>`;
}

export function describeRenderRegionMarkers(renderToString: (element: unknown) => string): void {
  const g = globalThis as BridgeGlobal;

  /**
   * Install the snapshot plus a content bridge whose per-specifier
   * renderers are supplied by the caller, and return the two entries.
   * `renderArtifacts` is the build-only switch the bundler emits into
   * `entry.mjs`; `undefined` models every non-build evaluation context.
   */
  function arrange(
    renderers: Record<string, (props: ContentProps) => unknown>,
    renderArtifacts: boolean | undefined,
  ): { outer: CollectionEntry; inner: CollectionEntry } {
    setContentSnapshot(SNAPSHOT);
    const ns = g.__zfb ?? {};
    ns.content = { get: (specifier) => renderers[specifier] };
    ns.renderArtifacts = renderArtifacts;
    g.__zfb = ns;
    const entries = getCollection("blog");
    const outer = entries.find((e) => e.slug === "outer");
    const inner = entries.find((e) => e.slug === "inner");
    if (!outer || !inner) throw new Error("fixture snapshot did not yield both entries");
    return { outer, inner };
  }

  afterEach(() => {
    setContentSnapshot(undefined);
    delete g.__zfb?.content;
    delete g.__zfb?.renderArtifacts;
  });

  describe("render-region markers", () => {
    it("wraps a rendered region in exactly one sentinel pair carrying the region id", () => {
      const { outer } = arrange({ [OUTER_ID]: () => jsx("p", { children: "region" }) }, true);

      const html = renderToString(jsx(outer.Content, {}));

      // Exact bytes: one pair, ids matching the entry's module specifier,
      // and NOTHING between a sentinel and the region — no space, no
      // newline, no empty text node.
      expect(html).toBe(`${marker("start", OUTER_ID)}<p>region</p>${marker("end", OUTER_ID)}`);
    });

    it("renders byte-identically to the bare renderer output when the switch is off", () => {
      const renderers = { [OUTER_ID]: () => jsx("p", { children: "region" }) };

      const off = renderToString(jsx(arrange(renderers, undefined).outer.Content, {}));
      const explicitlyFalse = renderToString(jsx(arrange(renderers, false).outer.Content, {}));

      // What the bridge's renderer produces on its own — the pre-#2421
      // output, obtained without going through `Content` at all.
      const bare = renderToString(jsx("p", { children: "region" }));
      expect(off).toBe(bare);
      expect(explicitlyFalse).toBe(bare);
      expect(off).not.toContain("data-zfb-render-region");
    });

    it("emits sibling pairs for repeated Content calls on one page", () => {
      const { outer } = arrange({ [OUTER_ID]: () => jsx("p", { children: "region" }) }, true);

      const html = renderToString(
        jsxs("div", { children: [jsx(outer.Content, {}), jsx(outer.Content, {})] }),
      );

      const one = `${marker("start", OUTER_ID)}<p>region</p>${marker("end", OUTER_ID)}`;
      expect(html).toBe(`<div>${one}${one}</div>`);
    });

    it("nests pairs when a Content renders another Content inside itself", () => {
      const { outer } = arrange(
        {
          // Looked up the way a page would, at render time — the outer
          // region's markup genuinely contains a second `Content` call.
          [OUTER_ID]: () => {
            const nested = getCollection("blog").find((e) => e.slug === "inner");
            if (!nested) throw new Error("nested entry missing from the snapshot");
            return jsx("section", { children: jsx(nested.Content, {}) });
          },
          [INNER_ID]: () => jsx("p", { children: "inner region" }),
        },
        true,
      );

      const html = renderToString(jsx(outer.Content, {}));

      expect(html).toBe(
        `${marker("start", OUTER_ID)}` +
          "<section>" +
          `${marker("start", INNER_ID)}<p>inner region</p>${marker("end", INNER_ID)}` +
          "</section>" +
          `${marker("end", OUTER_ID)}`,
      );
    });

    it("leaves the no-bridge fallback unwrapped", () => {
      // The markers wrap the RENDERER invocation, not the
      // `<pre data-zfb-content-fallback>` degraded path: a fallback means
      // the production renderer never ran, so there is no rendered region
      // for the artifact writer to capture. `strictContentBridge` is the
      // knob that turns that situation into a build failure.
      const { outer } = arrange({}, true);

      const html = renderToString(jsx(outer.Content, {}));

      expect(html).toContain("data-zfb-content-fallback");
      expect(html).not.toContain("data-zfb-render-region");
    });
  });
}
