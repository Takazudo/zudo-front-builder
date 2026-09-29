import { expect, it } from "vitest";
import { isDescription } from "@takazudo/zfb/zudo-react";
import { renderToString } from "@takazudo/zfb/zudo-react/server";

import { ClientRouter } from "../client-router-component.js";

it("emits owned head descriptions with trusted static styles", () => {
  const nodes = ClientRouter();
  expect(nodes.every(isDescription)).toBe(true);
  const html = renderToString(nodes as never);
  expect(html).toContain("<style>");
  expect(html).toContain(".zfb-route-announcer");
  expect(html).toContain('name="zfb-view-transitions-enabled"');
});
