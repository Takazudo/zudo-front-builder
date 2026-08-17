/**
 * Render-region markers under the React JSX runtime (epic #2421).
 *
 * No module swapping here: `content.ts` imports `react/jsx-runtime`
 * literally, which is what a React-mode project bundles. The oracle is
 * the real `react-dom/server` — `renderToStaticMarkup`, not
 * `renderToString`, so the output carries no hydration bookkeeping and
 * the byte assertions are about zfb's markup alone.
 *
 * The Preact half lives in `content-render-markers-preact.test.ts`.
 */

import { renderToStaticMarkup } from "react-dom/server";
import { Fragment } from "react/jsx-runtime";
import type { ReactElement } from "react";
import { expect, it } from "vitest";

import { describeRenderRegionMarkers } from "./render-region-marker-cases.js";

// `react-dom` is pinned to an EXACT version in this package's
// devDependencies rather than a range: `react-dom/server` refuses to load
// against a react of a different patch version, and `react` here is
// whatever the workspace already resolved. If a react bump ever makes this
// file fail to load, move the pin — do not widen it back to a range.

// Discriminator, mirroring the Preact file's: prove this file exercises the
// React runtime rather than accidentally sharing the sibling's mock.
it("really runs against the React JSX runtime", () => {
  expect(Fragment).toBe(Symbol.for("react.fragment"));
});

describeRenderRegionMarkers((element) => renderToStaticMarkup(element as ReactElement));
