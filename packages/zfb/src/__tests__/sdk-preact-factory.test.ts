// @vitest-environment node
import { render } from "preact-render-to-string";
import { jsx } from "preact/jsx-runtime";
import { expect, it, vi } from "vitest";

vi.mock("@takazudo/zfb/jsx-factory", async () => await import("preact/jsx-runtime"));

it("Island and ClientRouter render through the default Preact factory", async () => {
  const { Island } = await import("../island.js");
  const { ClientRouter } = await import("../../../zfb-runtime/src/client-router-component.js");
  function Card() {
    return jsx("span", { children: "preact" });
  }
  const islandHtml = render(Island({ children: jsx(Card, {}) }) as never);
  expect(islandHtml).toContain("preact");
  expect(islandHtml).toContain('data-zfb-island="Card"');
  const headHtml = render(ClientRouter() as never);
  expect(headHtml).toContain(".zfb-route-announcer");
  expect(headHtml).toContain('name="zfb-view-transitions-enabled"');
});
