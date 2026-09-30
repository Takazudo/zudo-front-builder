// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ISLAND_MOUNTED_ATTR, mountIslands, unmountIslands } from "../runtime.js";
import { mountTestIslands, mountNewTestIslands } from "./owned-manifest-fixture.js";

const persist = 'data-zfb-transition-persist="h"';
const attrs = 'data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1"';
const island = (name: string, props = "{}", extra = "", marker = "data-zfb-island") =>
  `<div ${marker}="${name}" ${attrs} data-zfb-build="test" data-props='${props}' ${extra}></div>`;
const body = (html: string) =>
  new DOMParser().parseFromString(`<body>${html}</body>`, "text/html").body;
const handle = (element: Element) =>
  (element as unknown as Record<symbol, unknown>)[Symbol.for("@takazudo/zfb/zudo-react/root-v1")];

beforeEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

function setup(oldHtml: string, names = ["A", "B", "C"]) {
  document.body.innerHTML = oldHtml;
  const mounts = Object.fromEntries(names.map((name) => [name, vi.fn()]));
  const disposals = Object.fromEntries(names.map((name) => [name, vi.fn()]));
  mountTestIslands(
    Object.fromEntries(
      names.map((name) => [
        name,
        {
          mount: mounts[name]!,
          dispose: disposals[name]!,
        },
      ]),
    ),
  );
  return { mounts, disposals };
}

function swapHeader(incoming: HTMLBodyElement) {
  const oldBoundary = document.body.querySelector(`[data-zfb-transition-persist="h"]`)!;
  const target = incoming.querySelector(`[data-zfb-transition-persist="h"]`)!;
  const oldCount = oldBoundary.querySelectorAll(
    "[data-zfb-island],[data-zfb-island-skip-ssr]",
  ).length;
  const newIslands = target.querySelectorAll("[data-zfb-island],[data-zfb-island-skip-ssr]");
  for (let index = oldCount; index < newIslands.length; index++) {
    oldBoundary.appendChild(newIslands[index]!.cloneNode(true));
  }
  mountNewTestIslands();
}

describe("unmountIslands persistence plan", () => {
  it("keeps an unchanged descendant's handle and mounted marker", () => {
    const { mounts, disposals } = setup(`<header ${persist}>${island("A")}</header>`);
    const old = document.querySelector("[data-zfb-island]")!;
    const originalHandle = handle(old);
    const incoming = body(`<header ${persist}>${island("A")}</header>`);
    unmountIslands(document.body, incoming);
    swapHeader(incoming);
    expect(handle(old)).toBe(originalHandle);
    expect(old.hasAttribute(ISLAND_MOUNTED_ATTR)).toBe(true);
    expect(disposals.A).not.toHaveBeenCalled();
    expect(mounts.A).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["props", island("A", '{"v":2}'), '{"v":2}'],
    [
      "build",
      island("A", '{"v":1}').replace('data-zfb-build="test"', 'data-zfb-build="other"'),
      '{"v":1}',
    ],
  ])(
    "flags a changed %s descendant, then disposes once and renders",
    (_case, next, expectedProps) => {
      const { mounts, disposals } = setup(`<header ${persist}>${island("A", '{"v":1}')}</header>`);
      const old = document.querySelector("[data-zfb-island]")!;
      const incoming = body(`<header ${persist}>${next}</header>`);
      unmountIslands(document.body, incoming);
      expect(old.hasAttribute("data-zfb-island-remount")).toBe(true);
      expect(old.getAttribute("data-props")).toBe(expectedProps);
      expect(disposals.A).not.toHaveBeenCalled();
      if (_case === "build") {
        // The post-navigation bundle supplies the incoming build identity.
        mountIslands({
          A: {
            identity: { component: "A", build: "other" },
            mount: (props, element, mode) => {
              mounts.A!(props, element, mode);
              return { dispose: () => disposals.A!(element), unmount: vi.fn() };
            },
          },
        });
      }
      swapHeader(incoming);
      expect(disposals.A).toHaveBeenCalledTimes(1);
      expect(mounts.A).toHaveBeenCalledTimes(2);
      expect(mounts.A!.mock.calls[1]![2]).toBe("render");
    },
  );

  it("pairs a component replacement at the same slot and renders the new component", () => {
    const { mounts, disposals } = setup(`<header ${persist}>${island("A")}</header>`);
    const old = document.querySelector("[data-zfb-island]")!;
    const incoming = body(`<header ${persist}>${island("B")}</header>`);
    unmountIslands(document.body, incoming);
    expect(old.getAttribute("data-zfb-island")).toBe("B");
    swapHeader(incoming);
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(mounts.B).toHaveBeenCalledWith({}, old, "render");
  });

  it("treats switching from SSR to skip SSR as changed", () => {
    const { mounts, disposals } = setup(`<header ${persist}>${island("A")}</header>`);
    const old = document.querySelector("[data-zfb-island]")!;
    const incoming = body(
      `<header ${persist}>${island("A", "{}", "", "data-zfb-island-skip-ssr")}</header>`,
    );
    unmountIslands(document.body, incoming);
    expect(old.hasAttribute("data-zfb-island")).toBe(false);
    expect(old.hasAttribute("data-zfb-island-skip-ssr")).toBe(true);
    swapHeader(incoming);
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(mounts.A!.mock.calls[1]![2]).toBe("render");
  });

  it("honours persist-props=true and retains the old props and handle", () => {
    const { disposals } = setup(
      `<header ${persist}>${island("A", '{"v":1}', 'data-zfb-transition-persist-props="true"')}</header>`,
    );
    const old = document.querySelector("[data-zfb-island]")!;
    const originalHandle = handle(old);
    const incoming = body(`<header ${persist}>${island("A", '{"v":2}')}</header>`);
    unmountIslands(document.body, incoming);
    expect(old.getAttribute("data-props")).toBe('{"v":1}');
    expect(old.hasAttribute("data-zfb-island-remount")).toBe(false);
    swapHeader(incoming);
    expect(handle(old)).toBe(originalHandle);
    expect(disposals.A).not.toHaveBeenCalled();
  });

  it("disposes and detaches a removed descendant", () => {
    const { disposals } = setup(`<header ${persist}>${island("A")}</header>`);
    const old = document.querySelector("[data-zfb-island]")!;
    unmountIslands(document.body, body(`<header ${persist}></header>`));
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(old.isConnected).toBe(false);
  });

  it("pairs same-component siblings by order; insertion leaves old handles alone", () => {
    const { mounts, disposals } = setup(
      `<header ${persist}>${island("A", '{"v":1}')}${island("A", '{"v":2}')}</header>`,
    );
    const [first, second] = [...document.querySelectorAll("[data-zfb-island]")];
    const incoming = body(
      `<header ${persist}>${island("A", '{"v":3}')}${island("A", '{"v":2}')}${island("A", '{"v":4}')}</header>`,
    );
    unmountIslands(document.body, incoming);
    expect(first!.hasAttribute("data-zfb-island-remount")).toBe(true);
    expect(second!.hasAttribute("data-zfb-island-remount")).toBe(false);
    swapHeader(incoming);
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(mounts.A).toHaveBeenCalledTimes(4);
  });

  it("removes only the unmatched same-component sibling", () => {
    const { disposals } = setup(
      `<header ${persist}>${island("A", '{"v":1}')}${island("A", '{"v":2}')}</header>`,
    );
    const [first, second] = [...document.querySelectorAll("[data-zfb-island]")];
    unmountIslands(document.body, body(`<header ${persist}>${island("A", '{"v":1}')}</header>`));
    expect(first!.isConnected).toBe(true);
    expect(second!.isConnected).toBe(false);
    expect(disposals.A).toHaveBeenCalledTimes(1);
  });

  it("recreates a persisted island root when its component changes", () => {
    const { mounts, disposals } = setup(
      `<div ${persist} ${attrs} data-zfb-island="A" data-zfb-build="test" data-props="{}"></div>`,
    );
    const old = document.body.firstElementChild!;
    const incoming = body(
      `<div ${persist} ${attrs} data-zfb-island="B" data-zfb-build="test" data-props="{}"></div>`,
    );
    unmountIslands(document.body, incoming);
    swapHeader(incoming);
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(mounts.B).toHaveBeenCalledWith({}, old, "render");
  });

  it("disposes a persisted island root whose target is no longer an island", () => {
    const { disposals } = setup(
      `<div ${persist} ${attrs} data-zfb-island="A" data-zfb-build="test" data-props="{}"></div>`,
    );
    const old = document.body.firstElementChild!;
    unmountIslands(document.body, body(`<div ${persist}></div>`));
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(old.isConnected).toBe(false);
  });

  it("disposes islands inside a nested matched persist whose target is inside another matched target", () => {
    const { disposals } = setup(
      `<div ${persist}></div><div data-zfb-transition-persist="inner">${island("A")}</div>`,
    );
    const incoming = body(
      `<div ${persist}><div data-zfb-transition-persist="inner">${island("A")}</div></div>`,
    );
    unmountIslands(document.body, incoming);
    expect(disposals.A).toHaveBeenCalledTimes(1);
  });

  it("retains a matched non-lifted descendant inside a lifted old ancestor", () => {
    const { disposals } = setup(
      `<header ${persist}><div data-zfb-transition-persist="inner">${island("A")}</div></header>`,
    );
    const old = document.querySelector("[data-zfb-island]")!;
    const originalHandle = handle(old);
    const incoming = body(
      `<header ${persist}><div data-zfb-transition-persist="inner">${island("A")}</div></header>`,
    );
    unmountIslands(document.body, incoming);
    expect(disposals.A).not.toHaveBeenCalled();
    expect(handle(old)).toBe(originalHandle);
  });

  it("honours a pre-set remount flag even when metadata is unchanged", () => {
    const { mounts, disposals } = setup(`<header ${persist}>${island("A")}</header>`);
    const old = document.querySelector("[data-zfb-island]")!;
    old.setAttribute("data-zfb-island-remount", "");
    const incoming = body(`<header ${persist}>${island("A")}</header>`);
    unmountIslands(document.body, incoming);
    expect(old.hasAttribute("data-zfb-island-remount")).toBe(true);
    swapHeader(incoming);
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(mounts.A!.mock.calls[1]![2]).toBe("render");
  });

  it("disposes every island with no incoming body", () => {
    const { disposals } = setup(`<header ${persist}>${island("A")}</header>${island("B")}`);
    unmountIslands(document.body);
    expect(disposals.A).toHaveBeenCalledTimes(1);
    expect(disposals.B).toHaveBeenCalledTimes(1);
  });
});
