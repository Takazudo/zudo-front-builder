// @vitest-environment node
/**
 * Render-region sentinel bytes through the owned zudo-react server renderer.
 * Vitest resolves the SDK factory alias to the same entry as the owned bundler.
 */
import { expect, it } from "vitest";

import { renderToString } from "../zudo-react/server.js";
import { Fragment as OwnedFragment } from "../zudo-react/jsx-runtime.js";
import { describeRenderRegionMarkers } from "./render-region-marker-cases.js";

// A JSX Fragment is renderable by several SSR libraries too, so bytes
// alone cannot establish that content.ts closed over the owned Fragment.
it("really runs against the owned zudo-react JSX runtime", async () => {
  const { Fragment } = await import("../zudo-react/jsx-runtime.js");
  expect(Fragment).toBe(OwnedFragment);
});

describeRenderRegionMarkers((element) => renderToString(element as never));
