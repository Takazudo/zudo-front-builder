// @vitest-environment node
/**
 * Render-region sentinel bytes through the owned zudo-react server renderer.
 * This vi.mock substitution is scoped to this test module graph, matching
 * the SDK factory alias selected by the owned bundler.
 */
import { expect, it, vi } from "vitest";

import { renderToString } from "../zudo-react/server.js";
import { Fragment as OwnedFragment } from "../zudo-react/jsx-runtime.js";
import { describeRenderRegionMarkers } from "./render-region-marker-cases.js";

vi.mock("@takazudo/zfb/jsx-factory", async () => await import("../zudo-react/jsx-runtime.js"));

// A React Fragment is renderable by several SSR libraries too, so bytes
// alone cannot establish that content.ts closed over the owned Fragment.
it("really runs against the owned zudo-react JSX runtime", async () => {
  const { Fragment } = await import("@takazudo/zfb/jsx-factory");
  expect(Fragment).toBe(OwnedFragment);
});

describeRenderRegionMarkers((element) => renderToString(element as never));
