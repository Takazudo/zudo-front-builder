import { beforeEach, describe, expect, it } from "vitest";
import { flush, h, signal } from "../../zudo-react/index.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const identity = { component: "Vocabulary", build: "b1" };
function Vocabulary({ tag, props }: { tag: string; props: Record<string, unknown> }) {
  return h("div", null, h(tag, props));
}
function containerFor(tag: string, props: Record<string, unknown> = {}): Element {
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Vocabulary, { tag, props }), { identity }));
  return host.firstElementChild!;
}
const cases: Array<[string, Record<string, unknown>, string]> = [
  [
    "meta",
    { property: "og:title", itemprop: "name", content: "Example" },
    '<meta property="og:title" itemprop="name" content="Example">',
  ],
  [
    "link",
    { rel: "preload", hreflang: "en", as: "font", integrity: "sha256-x" },
    '<link rel="preload" hreflang="en" as="font" integrity="sha256-x">',
  ],
  [
    "script",
    { async: true, defer: false, nonce: "abc", integrity: "sha256-x" },
    '<script async nonce="abc" integrity="sha256-x"></script>',
  ],
  [
    "button",
    { popovertarget: "menu", popovertargetaction: "toggle", form: "other" },
    '<button popovertarget="menu" popovertargetaction="toggle" form="other"></button>',
  ],
  ["div", { popover: "auto" }, '<div popover="auto"></div>'],
  ["iframe", { srcdoc: "<p>x</p>" }, '<iframe srcdoc="&lt;p&gt;x&lt;/p&gt;"></iframe>'],
  ["img", { fetchpriority: "high" }, '<img fetchpriority="high">'],
  [
    "input",
    { inputmode: "numeric", autocapitalize: "off", form: "other" },
    '<input inputmode="numeric" autocapitalize="off" form="other">',
  ],
  ["option", { label: "One" }, '<option label="One"></option>'],
  [
    "video",
    { preload: "metadata", playsinline: true },
    '<video preload="metadata" playsinline></video>',
  ],
  ["textarea", { wrap: "soft" }, '<textarea wrap="soft"></textarea>'],
  ["dialog", { closedby: "any" }, '<dialog closedby="any"></dialog>'],
  ["search", {}, "<search></search>"],
  ["hgroup", {}, "<hgroup></hgroup>"],
  ["menu", {}, "<menu></menu>"],
];

beforeEach(() => document.body.replaceChildren());

describe("finite HTML/SVG vocabulary", () => {
  it.each(cases)("serializes and mounts %s", (tag, props, html) => {
    expect(renderToString(h(tag, props))).toBe(html);
    if (tag === "iframe") return; // iframe remains outside island parser contexts.
    const container = containerFor(tag, props);
    const diagnostics: Diagnostic[] = [];
    const handle = mount(h(Vocabulary, { tag, props }), container, {
      identity,
      report: (item) => diagnostics.push(item),
    });
    expect(handle).not.toBeNull();
    expect(diagnostics).toEqual([]);
    handle?.dispose();
  });

  it("serializes categorized values and omissions", () => {
    expect(renderToString(h("a", { download: true }))).toBe('<a download=""></a>');
    expect(renderToString(h("a", { download: false }))).toBe("<a></a>");
    expect(renderToString(h("a", { download: "" }))).toBe('<a download=""></a>');
    expect(
      renderToString(h("input", { spellcheck: false, contenteditable: true, draggable: "false" })),
    ).toBe('<input spellcheck="false" contenteditable="true" draggable="false">');
    expect(renderToString(h("ol", { start: 4, reversed: false }))).toBe('<ol start="4"></ol>');
    expect(renderToString(h("script", { async: null, defer: false }))).toBe("<script></script>");
  });

  it("supports SVG names, case, and namespaced attributes", () => {
    const svg = h(
      "svg",
      {
        xmlns: "http://www.w3.org/2000/svg",
        "xmlns:xlink": "http://www.w3.org/1999/xlink",
        focusable: "false",
        viewBox: "0 0 10 10",
      },
      h(
        "defs",
        null,
        h("pattern", { id: "p" }),
        h("filter", { id: "f" }),
        h("marker", { id: "m" }),
      ),
      h("use", {
        href: "#p",
        "xlink:href": "#p",
        "fill-opacity": 0.5,
        "stroke-dasharray": "2 1",
        "text-anchor": "middle",
      }),
      h("image", { href: "/x.png" }),
    );
    const html = renderToString(svg);
    expect(html).toContain('viewBox="0 0 10 10"');
    expect(html).toContain('xlink:href="#p"');
    expect(html).toContain('<pattern id="p"></pattern>');
    function SvgVocabulary() {
      return h("div", null, svg);
    }
    const svgIdentity = { component: "SvgVocabulary", build: "b1" };
    const host = document.createElement("div");
    document.body.append(host);
    host.innerHTML = renderToString(islandRoot(h(SvgVocabulary, {}), { identity: svgIdentity }));
    const handle = mount(h(SvgVocabulary, {}), host.firstElementChild!, { identity: svgIdentity });
    expect(handle).not.toBeNull();
    const use = host.querySelector("use")!;
    expect(use.getAttributeNS("http://www.w3.org/1999/xlink", "href")).toBe("#p");
    expect(
      host.querySelector("svg")?.getAttributeNS("http://www.w3.org/2000/xmlns/", "xmlns"),
    ).toBe("http://www.w3.org/2000/svg");
    handle?.dispose();
  });

  it("uses the same metadata for hydration and reactive updates", async () => {
    const download = signal<string | boolean | null>(true);
    const spellcheck = signal<string | boolean>(false);
    const xlink = signal<string | null>("#one");
    function Demo() {
      return h(
        "div",
        null,
        h("a", { download, spellcheck }),
        h("svg", null, h("use", { "xlink:href": xlink })),
      );
    }
    const host = document.createElement("div");
    document.body.append(host);
    const demoIdentity = { component: "Demo", build: "b1" };
    host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity: demoIdentity }));
    const container = host.firstElementChild!;
    const diagnostics: Diagnostic[] = [];
    const handle = hydrate(h(Demo, {}), container, {
      identity: demoIdentity,
      report: (item) => diagnostics.push(item),
    });
    expect(handle).not.toBeNull();
    const a = container.querySelector("a")!;
    const use = container.querySelector("use")!;
    expect(a.getAttribute("download")).toBe("");
    expect(a.getAttribute("spellcheck")).toBe("false");
    download.value = false;
    spellcheck.value = true;
    xlink.value = null;
    await flush();
    expect(a.hasAttribute("download")).toBe(false);
    expect(a.getAttribute("spellcheck")).toBe("true");
    expect(use.hasAttributeNS("http://www.w3.org/1999/xlink", "href")).toBe(false);
    expect(diagnostics).toEqual([]);
    handle?.dispose();
  });

  it.each([
    ["meta", { property: false }],
    ["meta", { property: 1 }],
    ["link", { integrity: {} }],
    ["a", { download: 1 }],
    ["ol", { start: false }],
    ["script", { async: "true" }],
    ["svg", { focusable: true }],
    ["svg", { focusable: 1 }],
    ["div", { unknownattribute: "x" }],
    ["div", { className: "x" }],
    ["div", { onclick: () => {} }],
    ["site-card", { hidden: true }],
  ] as Array<[string, Record<string, unknown>]>)(
    "rejects invalid %s props in SSR and mount",
    (tag, props) => {
      expect(() => renderToString(h(tag, props))).toThrow(/ZR_(ATTRIBUTE|PROP_DIALECT)/);
      const container = containerFor("div");
      const diagnostics: Diagnostic[] = [];
      const handle = mount(h(Vocabulary, { tag, props }), container, {
        identity,
        report: (item) => diagnostics.push(item),
      });
      expect(handle).toBeNull();
      expect(diagnostics[0]?.code).toMatch(/ZR_(ATTRIBUTE|PROP_DIALECT)/);
    },
  );
});
