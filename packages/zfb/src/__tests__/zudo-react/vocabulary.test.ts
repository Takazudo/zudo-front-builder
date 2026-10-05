import { beforeEach, describe, expect, it } from "vite-plus/test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flush, h, signal } from "../../zudo-react/index.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import {
  attributeError,
  booleanAttrs,
  commonAttrs,
  dialectSuggestion,
  enumeratedBooleanAttrs,
  htmlAttrs,
  htmlTags,
  isDialectProp,
  numericAttrs,
  overloadedBooleanAttrs,
  svgAttrs,
  svgTags,
} from "../../zudo-react/vocabulary.js";
import type { Diagnostic, Signal } from "../../zudo-react/index.js";
// @ts-expect-error The private docs generator is exercised here without adding it to the runtime API.
import {
  extractVocabulary,
  generateReferencePages,
  validateRendererPolicies,
} from "../../../../../docs/scripts/generate-renderer-vocabulary.mjs";

const vocabularySource = readFileSync(
  resolve(process.cwd(), "src/zudo-react/vocabulary.ts"),
  "utf8",
);
const renderSource = readFileSync(resolve(process.cwd(), "src/zudo-react/render-html.ts"), "utf8");
const hydrateSource = readFileSync(resolve(process.cwd(), "src/zudo-react/hydrate.ts"), "utf8");

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

let liveMarkupSignals: {
  property: Signal<string | null>;
  preloadHref: Signal<string | null>;
  download: Signal<string | boolean | null>;
  spellcheck: Signal<string | boolean | null>;
  listStart: Signal<number | null>;
  iconHref: Signal<string | null>;
};

function StandardMarkupIsland() {
  const values = (liveMarkupSignals = {
    property: signal<string | null>("og:title"),
    preloadHref: signal<string | null>("/assets/body.woff2"),
    download: signal<string | boolean | null>(true),
    spellcheck: signal<string | boolean | null>(false),
    listStart: signal<number | null>(3),
    iconHref: signal<string | null>("#shape"),
  });

  return h(
    "section",
    null,
    h("meta", { property: values.property, content: "Standard markup" }),
    h("link", { rel: "preload", href: values.preloadHref, as: "font" }),
    h("script", { defer: true, nonce: "page-nonce" }),
    h("a", { download: values.download }),
    h("input", { spellcheck: values.spellcheck }),
    h("ol", { start: values.listStart, reversed: false }),
    h("ruby", null, h("rb", null, "base"), h("rt", null, "reading")),
    h(
      "svg",
      { xmlns: "http://www.w3.org/2000/svg", viewBox: "0 0 16 16" },
      h("use", { href: "#shape", "xlink:href": values.iconHref }),
    ),
  );
}

const markupIdentity = { component: "StandardMarkupIsland", build: "b1" };
const inlineIdentity = { component: "InlineHandlerIsland", build: "b1" };
let inlineHandlerSignal!: Signal<string | null>;
function InlineHandlerIsland() {
  inlineHandlerSignal = signal<string | null>('return "ready" & <');
  return h("button", { onclick: inlineHandlerSignal });
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
  ["rb", {}, "<rb></rb>"],
];

beforeEach(() => document.body.replaceChildren());

describe("finite HTML/SVG vocabulary", () => {
  it("keeps the generated reference in sync with the runtime source", () => {
    expect(() => validateRendererPolicies(renderSource, hydrateSource)).not.toThrow();
    const generated = extractVocabulary(vocabularySource);
    expect(new Set(generated.htmlTags)).toEqual(htmlTags);
    expect(new Set(generated.svgTags)).toEqual(svgTags);
    expect(new Set(generated.commonAttrs)).toEqual(commonAttrs);
    expect(new Set(generated.htmlAttrs)).toEqual(htmlAttrs);
    expect(new Set(generated.svgAttrs)).toEqual(svgAttrs);
    expect(new Set(generated.booleanAttrs)).toEqual(booleanAttrs);
    expect(new Set(generated.enumeratedBooleanAttrs)).toEqual(enumeratedBooleanAttrs);
    expect(new Set(generated.overloadedBooleanAttrs)).toEqual(overloadedBooleanAttrs);
    expect(new Set(generated.numericAttrs)).toEqual(numericAttrs);

    for (const name of generated.stringAttrs)
      expect(attributeError(name, 1, false), name).toBe("requires a string");

    for (const alias of generated.aliases) {
      for (const namespace of ["html", "svg"] as const) {
        const targetAllowed =
          alias.spelling === "rawHtml"
            ? namespace === "html"
            : generated.commonAttrs.includes(alias.spelling) ||
              (namespace === "svg"
                ? generated.svgAttrs.includes(alias.spelling)
                : generated.htmlAttrs.includes(alias.spelling));
        const namespaceMatches = !alias.namespace || alias.namespace === namespace;
        const legacy = generated.legacyDialect.includes(alias.name);
        expect(isDialectProp(alias.name, namespace, false)).toBe(
          legacy || (namespaceMatches && targetAllowed),
        );
        expect(dialectSuggestion(alias.name, namespace, false)).toBe(
          namespaceMatches && targetAllowed ? alias.spelling : undefined,
        );
      }
      if (!generated.legacyDialect.includes(alias.name)) {
        expect(isDialectProp(alias.name, "html", true)).toBe(false);
        expect(dialectSuggestion(alias.name, "html", true)).toBeUndefined();
      }
    }

    const generatedPages = generateReferencePages(vocabularySource);
    expect(generatedPages.size).toBe(2);
    for (const [path, expected] of generatedPages)
      expect(readFileSync(path, "utf8"), path).toBe(expected);
  });

  it("fails clearly when the source extraction shape or alias behavior changes", () => {
    expect(() =>
      extractVocabulary(vocabularySource.replace("const stringAttrs", "const stringAttributes")),
    ).toThrow(/words set stringAttrs/);
    expect(() =>
      extractVocabulary(
        vocabularySource.replace("export const htmlTags", "// export const htmlTags"),
      ),
    ).toThrow(/words set htmlTags/);
    expect(() =>
      extractVocabulary(
        vocabularySource.replace('className: { spelling: "class" },', 'className: "class",'),
      ),
    ).toThrow(/unsupported propAliases entry shape/);
    expect(() =>
      extractVocabulary(
        vocabularySource.replace(
          'event === "DoubleClick" ? "dblclick" : event.toLowerCase()',
          "event.toLowerCase()",
        ),
      ),
    ).toThrow(/alias suggestion policy changed/);
    expect(() =>
      validateRendererPolicies(
        renderSource.replace(
          "const custom = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/.test(tag);",
          "const custom = false;",
        ),
        hydrateSource,
      ),
    ).toThrow(/server tag\/namespace policy changed/);
  });

  it("renders the standard ruby base and reading elements", () => {
    expect(renderToString(h("ruby", {}, h("rb", {}, "base"), h("rt", {}, "reading")))).toBe(
      "<ruby><rb>base</rb><rt>reading</rt></ruby>",
    );
  });

  it.each(cases)("serializes and mounts %s", (tag, props, html) => {
    expect(renderToString(h(tag, props))).toBe(html);
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

  it.each(["hydrate", "mount"] as const)(
    "escapes and updates reactive lowercase inline event strings during %s",
    async (mode) => {
      expect(renderToString(h("button", { onclick: 'return "ready" & <' }))).toBe(
        '<button onclick="return &quot;ready&quot; &amp; &lt;"></button>',
      );

      const host = document.createElement("div");
      document.body.append(host);
      host.innerHTML = renderToString(
        islandRoot(h(InlineHandlerIsland, {}), { identity: inlineIdentity }),
      );
      const container = host.firstElementChild!;
      if (mode === "mount") container.replaceChildren();
      const diagnostics: Diagnostic[] = [];
      const node = h(InlineHandlerIsland, {});
      const handle =
        mode === "hydrate"
          ? hydrate(node, container, {
              identity: inlineIdentity,
              report: (item) => diagnostics.push(item),
            })
          : mount(node, container, {
              identity: inlineIdentity,
              report: (item) => diagnostics.push(item),
            });

      expect(handle).not.toBeNull();
      expect(container.querySelector("button")?.getAttribute("onclick")).toBe('return "ready" & <');
      inlineHandlerSignal.value = 'return "updated" & >';
      await flush();
      expect(container.querySelector("button")?.getAttribute("onclick")).toBe(
        'return "updated" & >',
      );
      expect(diagnostics).toEqual([]);
      handle?.dispose();
    },
  );

  it.each(["hydrate", "mount"] as const)(
    "round-trips the standard markup matrix through SSR and %s",
    async (mode) => {
      const node = h(StandardMarkupIsland, {});
      const html = renderToString(islandRoot(node, { identity: markupIdentity }));
      expect(html).toContain('<meta property="og:title" content="Standard markup">');
      expect(html).toContain('<link rel="preload" href="/assets/body.woff2" as="font">');
      expect(html).toContain('<script defer nonce="page-nonce"></script>');
      expect(html).toContain("<ruby><rb>base</rb><rt>reading</rt></ruby>");
      expect(html).toContain('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">');
      expect(html).toContain('<use href="#shape" xlink:href="#shape"></use>');

      const host = document.createElement("div");
      document.body.append(host);
      host.innerHTML = renderToString(islandRoot(node, { identity: markupIdentity }));
      const container = host.firstElementChild!;
      if (mode === "mount") container.replaceChildren();

      const diagnostics: Diagnostic[] = [];
      const handle =
        mode === "hydrate"
          ? hydrate(h(StandardMarkupIsland, {}), container, {
              identity: markupIdentity,
              report: (item) => diagnostics.push(item),
            })
          : mount(h(StandardMarkupIsland, {}), container, {
              identity: markupIdentity,
              report: (item) => diagnostics.push(item),
            });

      expect(handle).not.toBeNull();
      expect(container.querySelector("meta")?.getAttribute("property")).toBe("og:title");
      expect(container.querySelector("link")?.getAttribute("as")).toBe("font");
      expect(container.querySelector("script")?.hasAttribute("defer")).toBe(true);
      expect(container.querySelector("a")?.getAttribute("download")).toBe("");
      expect(container.querySelector("input")?.getAttribute("spellcheck")).toBe("false");
      expect(container.querySelector("ol")?.getAttribute("start")).toBe("3");
      expect(container.querySelector("ruby")?.innerHTML).toBe("<rb>base</rb><rt>reading</rt>");
      expect(container.querySelector("svg")?.namespaceURI).toBe("http://www.w3.org/2000/svg");
      const use = container.querySelector("use")!;
      expect(use.getAttribute("xlink:href")).toBe("#shape");
      if (mode === "mount") {
        expect(use.getAttributeNS("http://www.w3.org/1999/xlink", "href")).toBe("#shape");
      }

      liveMarkupSignals.property.value = null;
      liveMarkupSignals.preloadHref.value = null;
      liveMarkupSignals.download.value = false;
      liveMarkupSignals.spellcheck.value = true;
      liveMarkupSignals.listStart.value = 4;
      if (mode === "mount") liveMarkupSignals.iconHref.value = null;
      await flush();

      expect(container.querySelector("meta")?.hasAttribute("property")).toBe(false);
      expect(container.querySelector("link")?.hasAttribute("href")).toBe(false);
      expect(container.querySelector("a")?.hasAttribute("download")).toBe(false);
      expect(container.querySelector("input")?.getAttribute("spellcheck")).toBe("true");
      expect(container.querySelector("ol")?.getAttribute("start")).toBe("4");
      if (mode === "mount") {
        expect(use.hasAttributeNS("http://www.w3.org/1999/xlink", "href")).toBe(false);
      }

      liveMarkupSignals.download.value = "";
      await flush();
      expect(container.querySelector("a")?.getAttribute("download")).toBe("");
      expect(diagnostics).toEqual([]);
      handle?.dispose();
    },
  );

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

  it("rejects an unknown standard tag", () => {
    expect(() => renderToString(h("madeuptag", null))).toThrow(/ZR_TAG/);

    const container = containerFor("div");
    const diagnostics: Diagnostic[] = [];
    const handle = mount(h(Vocabulary, { tag: "madeuptag", props: {} }), container, {
      identity,
      report: (item) => diagnostics.push(item),
    });
    expect(handle).toBeNull();
    expect(diagnostics[0]?.code).toBe("ZR_TAG");
  });
});
