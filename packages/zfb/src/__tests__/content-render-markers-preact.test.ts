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
 * The explicit identity check below ensures the mock remains in place:
 * a React element has a renderable shape too, so output assertions alone
 * would not prove that the Preact module replacement happened.
 */

import { render } from "preact-render-to-string";
import { Fragment as PreactFragment } from "preact/jsx-runtime";
import type { VNode } from "preact";
import { expect, it, vi } from "vitest";

import { describeRenderRegionMarkers } from "./render-region-marker-cases.js";

vi.mock("react/jsx-runtime", async () => await import("preact/jsx-runtime"));

// Pin the replacement itself instead of trusting rendered output: the
// `Fragment` `content.ts` closes over must BE Preact's.
it("really runs against the Preact JSX runtime", async () => {
  const { Fragment } = await import("react/jsx-runtime");
  expect(Fragment).toBe(PreactFragment);
});

describeRenderRegionMarkers((element) => render(element as VNode));
