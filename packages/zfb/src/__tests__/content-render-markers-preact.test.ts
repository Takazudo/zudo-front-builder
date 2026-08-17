/**
 * Render-region markers under the Preact JSX runtime (epic #2421).
 *
 * A Preact-mode project never resolves `react/jsx-runtime`: the engine
 * passes `--alias:react/jsx-runtime=preact/jsx-runtime` to esbuild
 * (`crates/zfb-build/src/bundler.rs`), so `content.ts`'s `Fragment` /
 * `jsx` / `jsxs` are Preact's. The `vi.mock` below reproduces exactly
 * that rewrite — same specifier, same replacement — for the whole module
 * graph of this file, and the oracle is the real
 * `preact-render-to-string`, the SSR entry point the Preact adapter pins.
 *
 * Without this file, "the markers render correctly" would be a claim
 * about React only; a Fragment imported from the wrong runtime renders as
 * an unknown component (or throws) rather than disappearing.
 */

import { render } from "preact-render-to-string";
import { Fragment as PreactFragment } from "preact/jsx-runtime";
import type { VNode } from "preact";
import { expect, it, vi } from "vitest";

import { describeRenderRegionMarkers } from "./render-region-marker-cases.js";

vi.mock("react/jsx-runtime", async () => await import("preact/jsx-runtime"));

// Discriminator. A React element is structurally `{ type, props, … }` too,
// so `preact-render-to-string` would happily render one — meaning a `vi.mock`
// that silently stopped applying would leave every assertion below passing
// while testing React a second time. Pin the swap itself instead of trusting
// it: the `Fragment` `content.ts` closes over must BE Preact's.
it("really runs against the Preact JSX runtime", async () => {
  const { Fragment } = await import("react/jsx-runtime");
  expect(Fragment).toBe(PreactFragment);
});

describeRenderRegionMarkers((element) => render(element as VNode));
