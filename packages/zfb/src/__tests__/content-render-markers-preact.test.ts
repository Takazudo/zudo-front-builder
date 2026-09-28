/**
 * Render-region markers under the Preact JSX runtime (epic #2421).
 *
 * A Preact-mode project resolves the SDK factory through the default
 * Preact alias. The `vi.mock` below selects the same factory for this module
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

vi.mock("@takazudo/zfb/jsx-factory", async () => await import("preact/jsx-runtime"));

// Pin the replacement itself instead of trusting rendered output: the
// `Fragment` `content.ts` closes over must BE Preact's.
it("really runs against the Preact JSX runtime", async () => {
  const { Fragment } = await import("@takazudo/zfb/jsx-factory");
  expect(Fragment).toBe(PreactFragment);
});

describeRenderRegionMarkers((element) => render(element as VNode));
