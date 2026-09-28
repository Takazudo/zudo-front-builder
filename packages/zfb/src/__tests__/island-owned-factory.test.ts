// @vitest-environment node
import { expect, it, vi } from "vitest";
import { Window } from "happy-dom";
import { jsx } from "../zudo-react/jsx-runtime.js";
import { isDescription } from "../zudo-react/index.js";
import { renderToString } from "../zudo-react/server.js";

vi.mock("@takazudo/zfb/jsx-factory", async () => await import("../zudo-react/jsx-runtime.js"));
vi.mock(
  "@takazudo/zfb/island-boundary",
  async () => await import("../island-boundary-zudo-react.js"),
);

it("Island mints its wrapper through the owned factory", async () => {
  (
    globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild: string; zudoReactIslands: string[] };
    }
  ).__zfb = {
    zudoReactBuild: "b1",
    zudoReactIslands: ["Card"],
  };
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", { children: "owned" });
  }
  const wrapper = Island({ children: jsx(Card, {}) });
  expect(isDescription(wrapper)).toBe(true);
  expect(renderToString(wrapper as never)).toContain('data-zfb-island="Card"');
});

it("rejects a conflicting displayName against scanner identity", async () => {
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", {});
  }
  (Card as typeof Card & { displayName?: string }).displayName = "Wrong";
  (
    globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild: string; zudoReactIslands: string[] };
    }
  ).__zfb = {
    zudoReactBuild: "b1",
    zudoReactIslands: ["Card", "Wrong"],
  };
  expect(() => Island({ children: jsx(Card, {}) })).toThrow(
    "Card conflicts with displayName Wrong",
  );
});

it("requires scanner metadata before constructing an owned island", async () => {
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", {});
  }
  (globalThis as typeof globalThis & { __zfb?: { zudoReactBuild: string } }).__zfb = {
    zudoReactBuild: "b1",
  };
  expect(() => Island({ children: jsx(Card, {}) })).toThrow("no scanner identity metadata");
  (
    globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild: string; zudoReactIslands: string[] };
    }
  ).__zfb = {
    zudoReactBuild: "b1",
    zudoReactIslands: ["Card"],
  };
});

it("rejects unsupported props with the component and path", async () => {
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", { children: "card" });
  }
  const invalid: Record<string, unknown> = {
    action: () => {},
    symbol: Symbol(),
    missing: undefined,
    big: 1n,
    number: Infinity,
    date: new Date(),
    instance: new (class Custom {})(),
    description: jsx("b", {}),
  };
  const cycle: Record<string, unknown> = {};
  cycle.self = cycle;
  invalid.cycle = cycle;
  for (const [key, value] of Object.entries(invalid)) {
    const wrapper = Island({ children: jsx(Card, { [key]: value }) });
    expect(() => renderToString(wrapper as never)).toThrow(new RegExp(`Card:.*props\\.${key}`));
  }
});

it("round-trips script-like and attribute-sensitive strings", async () => {
  const { Island } = await import("../island.js");
  function Card({ value }: { value: string }) {
    return jsx("span", { children: value });
  }
  for (const value of ["</script", '" &']) {
    const html = renderToString(Island({ children: jsx(Card, { value }) }) as never);
    const window = new Window();
    window.document.body.innerHTML = html;
    const encoded = window.document.body.firstElementChild?.getAttribute("data-props");
    expect(encoded).toBeDefined();
    expect(JSON.parse(encoded!)).toEqual({ value });
  }
});

it("rejects several children and a nested boundary", async () => {
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", {});
  }
  expect(() => Island({ children: [jsx(Card, {}), jsx(Card, {})] })).toThrow("ZR_ISLAND_CHILD");
  const inner = Island({ children: jsx(Card, {}) });
  const outer = Island({ children: jsx(Card, { children: inner }) });
  expect(() => renderToString(outer as never)).toThrow("ZR_ISLAND_CHILD");
});
