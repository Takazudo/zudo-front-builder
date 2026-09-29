import { beforeEach, describe, expect, it, vi } from "vitest";
import { h, signal, flush, getScope } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { subscriberCount } from "../../zudo-react/reactive.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const identity = { component: "Demo", build: "b1" };
let container: Element;
let diagnostics: Diagnostic[];
const options = () => ({ identity, report: (item: Diagnostic) => diagnostics.push(item) });
function server(node: ReturnType<typeof h>): Element {
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(node, { identity }));
  return host.firstElementChild!;
}
beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("hydrate", () => {
  it("adopts SVG dimensions accepted by the server", () => {
    function Demo() {
      return h("svg", { width: "16", height: 24 }, h("path", { d: "M0 0" }));
    }
    container = server(h(Demo, {}));
    expect(hydrate(h(Demo, {}), container, options())).not.toBeNull();
    expect(container.querySelector("svg")?.getAttribute("width")).toBe("16");
    expect(container.querySelector("svg")?.getAttribute("height")).toBe("24");
    expect(diagnostics).toEqual([]);
  });
  it("rejects a boolean SVG dimension during mount", () => {
    const width = signal<string | boolean>("16");
    function Demo() {
      return h("svg", { width });
    }
    container = server(h(Demo, {}));
    width.value = true;
    expect(mount(h(Demo, {}), container, options())).toBeNull();
    expect(diagnostics[0]?.code).toBe("ZR_ATTRIBUTE");
  });
  it("adopts server elements and text; mount replaces equal nodes", async () => {
    const text = signal("hello");
    function Demo() {
      return h("p", { id: "x", children: [text, h("b", { children: "world" })] });
    }
    container = server(h(Demo, {}));
    const paragraph = container.querySelector("p")!;
    const bold = container.querySelector("b")!;
    const slot = paragraph.childNodes[1]!;
    const handle = hydrate(h(Demo, {}), container, options());
    expect(handle).not.toBeNull();
    expect(container.querySelector("p")).toBe(paragraph);
    expect(container.querySelector("b")).toBe(bold);
    expect(paragraph.childNodes[1]).toBe(slot);
    text.value = "changed";
    await flush();
    expect(slot.textContent).toBe("changed");
    expect(diagnostics).toEqual([]);
    const other = server(h(Demo, {}));
    const old = other.querySelector("p");
    expect(mount(h(Demo, {}), other, options())).not.toBeNull();
    expect(other.querySelector("p")).not.toBe(old);
  });
  it.each([
    [
      "wrong tag",
      (root: Element) => root.querySelector("p")!.replaceWith(document.createElement("section")),
    ],
    ["missing marker", (root: Element) => root.firstChild!.remove()],
    [
      "wrong text",
      (root: Element) => {
        root.querySelector("p")!.firstChild!.textContent = "bad";
      },
    ],
    ["wrong identity", (root: Element) => root.setAttribute("data-zfb-build", "old")],
  ])("fails closed for %s", (_name, mutate) => {
    const state = signal("yes");
    const click = vi.fn();
    function Demo() {
      return h("p", { "on:click": click, children: state });
    }
    container = server(h(Demo, {}));
    mutate(container);
    const before = container.innerHTML;
    const listenerSpy = vi.spyOn(Element.prototype, "addEventListener");
    expect(hydrate(h(Demo, {}), container, options())).toBeNull();
    expect(container.innerHTML).toBe(before);
    expect(subscriberCount(state)).toBe(0);
    expect(listenerSpy).not.toHaveBeenCalled();
    expect(diagnostics).toHaveLength(1);
    expect(diagnostics[0]).toMatchObject({
      phase: "preflight",
      protocol: "zudo-react/1",
      build: "b1",
    });
    expect(diagnostics[0]!.path).toContain("div");
    listenerSpy.mockRestore();
  });
  it("normalizes ordinary whitespace", () => {
    function Demo() {
      return h("p", { children: "A   B\n C" });
    }
    container = server(h(Demo, {}));
    container.querySelector("p")!.textContent = "A B C";
    expect(hydrate(h(Demo, {}), container, options())).not.toBeNull();
  });
  it("assigns refs before activation, runs setup once, and disposes idempotently", async () => {
    const state = signal("a");
    const ref = { current: null as Element | null };
    const setup = vi.fn();
    const active = vi.fn(() => expect(ref.current).toBeInstanceOf(Element));
    function Demo() {
      setup();
      getScope().onActivate(active);
      return h("p", { ref, children: state });
    }
    container = server(h(Demo, {}));
    setup.mockClear();
    const handle = hydrate(h(Demo, {}), container, options())!;
    expect(setup).toHaveBeenCalledTimes(1);
    expect(active).toHaveBeenCalledTimes(1);
    expect(ref.current).toBe(container.querySelector("p"));
    expect(subscriberCount(state)).toBe(1);
    const old = container.innerHTML;
    handle.dispose();
    handle.dispose();
    expect(ref.current).toBeNull();
    expect(container.innerHTML).toBe(old);
    expect(subscriberCount(state)).toBe(0);
    state.value = "b";
    await flush();
    expect(container.innerHTML).toBe(old);
    handle.unmount();
    handle.unmount();
    expect(container.innerHTML).toBe("");
  });
  it("rejects a second root and leaves the first active", () => {
    function Demo() {
      return h("p", { children: "A" });
    }
    container = server(h(Demo, {}));
    const first = hydrate(h(Demo, {}), container, options());
    expect(hydrate(h(Demo, {}), container, options())).toBeNull();
    expect(diagnostics.at(-1)?.code).toBe("ZR_DUPLICATE_ROOT");
    expect(first?.disposed).toBe(false);
  });
  it("keeps dirty input value and focus", () => {
    function Demo() {
      return h("input", { type: "text", value: "server" });
    }
    container = server(h(Demo, {}));
    const input = container.querySelector("input")!;
    input.value = "typed";
    input.focus();
    expect(hydrate(h(Demo, {}), container, options())).not.toBeNull();
    expect(input.value).toBe("typed");
    expect(document.activeElement).toBe(input);
  });
  it("cancels between setup and commit without attaching resources", () => {
    const state = signal("x");
    const fresh = new AbortController();
    let clientPass = false;
    function ClientDemo() {
      if (clientPass) fresh.abort();
      return h("p", { children: state });
    }
    const host = document.createElement("div");
    document.body.append(host);
    host.innerHTML = renderToString(
      islandRoot(h(ClientDemo, {}), { identity: { ...identity, component: "ClientDemo" } }),
    );
    const root = host.firstElementChild!;
    clientPass = true;
    const before = root.innerHTML;
    const report: Diagnostic[] = [];
    expect(
      hydrate(h(ClientDemo, {}), root, {
        identity: { ...identity, component: "ClientDemo" },
        signal: fresh.signal,
        report: (d) => report.push(d),
      }),
    ).toBeNull();
    expect(root.innerHTML).toBe(before);
    expect(subscriberCount(state)).toBe(0);
    expect(report.at(-1)?.code).toBe("ZR_CANCELLED");
  });
  it("treats raw HTML as opaque while preserving its region", () => {
    function Demo() {
      return h("div", { rawHtml: "<em>server</em>" });
    }
    container = server(h(Demo, {}));
    container.querySelector("em")!.textContent = "client-altered";
    const em = container.querySelector("em");
    expect(hydrate(h(Demo, {}), container, options())).not.toBeNull();
    expect(container.querySelector("em")).toBe(em);
  });
  it("cancels aborted and disconnected roots", () => {
    function Demo() {
      return h("p", { children: "x" });
    }
    const controller = new AbortController();
    controller.abort();
    container = server(h(Demo, {}));
    expect(hydrate(h(Demo, {}), container, { ...options(), signal: controller.signal })).toBeNull();
    expect(diagnostics.at(-1)?.code).toBe("ZR_CANCELLED");
    container.remove();
    expect(hydrate(h(Demo, {}), container, options())).toBeNull();
    expect(diagnostics.at(-1)?.code).toBe("ZR_CANCELLED");
  });
});

describe.each(["hydrate", "mount"] as const)("%s reactive rawHtml", (mode) => {
  const attach = mode === "hydrate" ? hydrate : mount;
  function expectChildren(parent: Element, nodes: Node[]): void {
    expect(parent.childNodes).toHaveLength(nodes.length);
    nodes.forEach((node, index) => expect(parent.childNodes[index]).toBe(node));
  }
  it.each([
    ["null", null],
    ["number", 42],
    ["boolean", false],
    ["object", { toString: () => "<b>coerced</b>" }],
    ["opening marker", "<!--zr:1:9:h--><b>reserved</b><!--/zr:1:9-->"],
    ["closing marker", "<!--/zr:1:1--><b>reserved</b>"],
    ["island wrapper", '<div data-zfb-island="Nested">reserved</div>'],
    ["skip-SSR wrapper", '<div data-zfb-island-skip-ssr="Nested">reserved</div>'],
  ])("rejects a later %s without changing DOM and recovers", async (_, invalid) => {
    const initial = "<em>safe</em>";
    const raw = signal<unknown>(initial);
    const text = signal("before");
    function Demo() {
      return h("section", null, h("div", { rawHtml: raw }), h("span", null, text));
    }
    container = server(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, options());
    expect(handle).not.toBeNull();
    const region = container.querySelector("section > div")!;
    const original = [...region.childNodes];
    const before = region.innerHTML;

    raw.value = invalid;
    text.value = "after";
    await flush();
    expect(region.innerHTML).toBe(before);
    expectChildren(region, original);
    expect(container.querySelector("span")?.textContent).toBe("after");
    expect(diagnostics).toHaveLength(1);
    expect(diagnostics[0]).toMatchObject({
      code: "ZR_SUBSCRIBER_ERROR",
      phase: "update",
      actual: `Error: ZR_RAW_HTML: valid rawHtml; got ${typeof invalid}`,
    });
    expect(subscriberCount(raw)).toBe(1);

    // Restoring the last accepted value must preserve its existing opaque nodes.
    raw.value = initial;
    await flush();
    expectChildren(region, original);
    raw.value = "<strong>recovered</strong>";
    await flush();
    expect(region.querySelector("strong")?.textContent).toBe("recovered");
    expect(region.querySelector("em")).toBeNull();
    expect(region.firstChild).toBe(original[0]);
    expect(region.lastChild).toBe(original.at(-1));
    expect(region.childNodes).toHaveLength(3);
    expect(diagnostics).toHaveLength(1);
    handle!.dispose();
    expect(subscriberCount(raw)).toBe(0);
  });

  it("preserves opaque content for equal values and retains markers through empty updates", async () => {
    const raw = signal("<em>initial</em>");
    function Demo() {
      return h("div", { rawHtml: raw });
    }
    container = server(h(Demo, {}));
    container.querySelector("em")!.textContent = "opaque server content";
    const handle = attach(h(Demo, {}), container, options());
    expect(handle).not.toBeNull();
    const region = container.querySelector("div")!;
    const original = [...region.childNodes];
    expect(region.querySelector("em")?.textContent).toBe(
      mode === "hydrate" ? "opaque server content" : "initial",
    );
    raw.value = "<em>initial</em>";
    await flush();
    expectChildren(region, original);

    raw.value = "";
    await flush();
    expectChildren(region, [original[0]!, original.at(-1)!]);
    raw.value = "text<!--ordinary comment--><b>next</b>";
    await flush();
    expect(region.firstChild).toBe(original[0]);
    expect(region.lastChild).toBe(original.at(-1));
    expect(region.textContent).toBe("textnext");
    expect(region.childNodes).toHaveLength(5);
    expect(diagnostics).toEqual([]);
    handle!.dispose();
  });

  it("uses the adopted marker identities instead of searching comment text", async () => {
    const raw = signal("<em>initial</em>");
    function Demo() {
      return h("div", { rawHtml: raw });
    }
    container = server(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, options());
    expect(handle).not.toBeNull();
    const region = container.querySelector("div")!;
    const open = region.firstChild!;
    const close = region.lastChild!;
    const before = document.createComment("external:h");
    const after = document.createTextNode("outside");
    const impostor = document.createComment(close.textContent!);
    region.insertBefore(before, open);
    region.insertBefore(impostor, close);
    region.appendChild(after);

    raw.value = "<b>next</b>";
    await flush();
    expectChildren(region, [before, open, region.querySelector("b")!, close, after]);
    expect(impostor.parentNode).toBeNull();
    expect(diagnostics).toEqual([]);
    handle!.dispose();
  });

  it.each(["detached", "reordered"] as const)(
    "rejects a %s closing marker before removing any content",
    async (position) => {
      const raw = signal("<em>initial</em>");
      function Demo() {
        return h("div", { rawHtml: raw });
      }
      container = server(h(Demo, {}));
      const handle = attach(h(Demo, {}), container, options());
      expect(handle).not.toBeNull();
      const region = container.querySelector("div")!;
      const close = region.lastChild!;
      if (position === "detached") region.removeChild(close);
      else region.insertBefore(close, region.firstChild);
      const remaining = [...region.childNodes];
      raw.value = "<b>rejected</b>";
      await flush();
      expectChildren(region, remaining);
      expect(diagnostics[0]).toMatchObject({
        code: "ZR_SUBSCRIBER_ERROR",
        phase: "update",
        actual: "Error: ZR_REGION_RANGE",
      });
      region.appendChild(close);
      raw.value = "<b>recovered</b>";
      await flush();
      expect(region.querySelector("b")?.textContent).toBe("recovered");
      expect(region.firstChild).toBe(remaining[position === "detached" ? 0 : 1]);
      expect(region.lastChild).toBe(close);
      expect(diagnostics).toHaveLength(1);
      handle!.dispose();
    },
  );
});
