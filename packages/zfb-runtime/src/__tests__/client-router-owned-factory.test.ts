// @vitest-environment node
import { expect, it, vi } from "vitest";
import { isDescription } from "../../../zfb/src/zudo-react/index.js";
import { renderToString } from "../../../zfb/src/zudo-react/server.js";

vi.mock(
  "@takazudo/zfb/jsx-factory",
  async () => await import("../../../zfb/src/zudo-react/jsx-runtime.js"),
);

it("ClientRouter mints its head descriptions through the owned factory", async () => {
  const { ClientRouter } = await import("../client-router-component.js");
  const nodes = ClientRouter();
  expect(nodes).toHaveLength(3);
  expect(nodes.every(isDescription)).toBe(true);
  expect(nodes.map((node) => node.type)).toEqual(["style", "meta", "meta"]);
  const html = renderToString(nodes as never);
  expect(html).toContain("<style>");
  expect(html).toContain(".zfb-route-announcer");
  expect(html).toContain('name="zfb-view-transitions-enabled"');
});
