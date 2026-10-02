// @vitest-environment node
import { expect, it } from "vitest";
import { Window } from "happy-dom";
import { jsx } from "../zudo-react/jsx-runtime.js";
import { isDescription } from "../zudo-react/index.js";
import { renderToString } from "../zudo-react/server.js";

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

it("serializes only supported persistence options on an owned island", async () => {
  const { Island } = await import("../island.js");
  function Card() {
    return jsx("span", { children: "owned" });
  }
  const render = (options: { persist?: string; persistProps?: boolean }) =>
    renderToString(Island({ children: jsx(Card, {}), ...options }) as never);
  expect(render({ persist: 'sidebar"&' })).toContain(
    'data-zfb-transition-persist="sidebar&quot;&amp;"',
  );
  expect(render({ persist: "sidebar", persistProps: true })).toContain(
    'data-zfb-transition-persist-props="true"',
  );
  expect(render({ persist: "sidebar", persistProps: false })).not.toContain(
    "data-zfb-transition-persist-props",
  );
  for (const persist of ["", " ", " sidebar", "sidebar\nother"]) {
    expect(() => render({ persist })).toThrow("ZR_ISLAND_PERSIST");
  }
  expect(() => render({ persistProps: true })).toThrow("persistProps requires persist");
  expect(() => render({ persistProps: false })).toThrow("persistProps requires persist");
});

it("keeps persistence options beside SDK-owned identity on a deferred skip-SSR island", async () => {
  (
    globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild: string; zudoReactIslands: string[] };
    }
  ).__zfb = {
    zudoReactBuild: "b1",
    zudoReactIslands: ["Card"],
  };
  const { Island } = await import("../island.js");
  function Card({ count }: { count: number }) {
    return jsx("b", { children: count });
  }
  const html = renderToString(
    Island({
      children: jsx(Card, { count: 3 }),
      when: "media",
      media: "(min-width: 40rem)",
      persist: "sidebar-tree",
      persistProps: true,
      ssrFallback: jsx("span", { children: "pending" }),
    }) as never,
  );

  expect(html).toBe(
    '<div data-zfb-island-skip-ssr="Card" data-when="media" data-media="(min-width: 40rem)" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{&quot;count&quot;:3}" data-zfb-transition-persist="sidebar-tree" data-zfb-transition-persist-props="true"><span>pending</span></div>',
  );
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

it("omits undefined props from both owned island SSR and payload", async () => {
  const { Island } = await import("../island.js");
  const nested = { missing: undefined, present: null };
  function Card({ nested: value }: { nested: typeof nested }) {
    return jsx("span", {
      children: `${Object.hasOwn(value, "missing")}:${Object.hasOwn(value, "present")}`,
    });
  }
  const html = renderToString(Island({ children: jsx(Card, { nested }) }) as never);
  const window = new Window();
  window.document.body.innerHTML = html;
  const wrapper = window.document.body.firstElementChild;
  expect(wrapper?.getAttribute("data-props")).toBe('{"nested":{"present":null}}');
  expect(wrapper?.textContent).toBe("false:true");
  expect(Object.hasOwn(nested, "missing")).toBe(true);
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
