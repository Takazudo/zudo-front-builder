// @vitest-environment node
import { expect, it, vi } from "vitest";
import { jsx } from "../zudo-react/jsx-runtime.js";
import { isDescription } from "../zudo-react/index.js";

vi.mock("@takazudo/zfb/jsx-factory", async () => await import("../zudo-react/jsx-runtime.js"));

it("Island mints its wrapper through the owned factory", async () => {
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", { children: "owned" });
  }
  const wrapper = Island({ children: jsx(Card, {}) });
  expect(isDescription(wrapper)).toBe(true);
  expect(wrapper.type).toBe("div");
  expect(wrapper.props["data-zfb-island"]).toBe("Card");
});
