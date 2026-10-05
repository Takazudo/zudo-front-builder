import { beforeEach, describe, expect, it } from "vite-plus/test";
import { h } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { rawHtmlReserved } from "../../zudo-react/raw-html.js";
import type { Diagnostic } from "../../zudo-react/index.js";

// Display-only payloads (#3570): marker-like text that cannot form a reserved
// attribute or protocol comment, paired with the exact visible text.
const accepted: [string, string][] = [
  ["data-zfb-island=&quot;Demo&quot;", 'data-zfb-island="Demo"'],
  [
    `grep -roh 'data-zfb-island="[^"]*"' dist/ | sort -u`,
    `grep -roh 'data-zfb-island="[^"]*"' dist/ | sort -u`,
  ],
  [
    '<span style="color:#a3be8c;">&#39;data-zfb-island=&quot;[^&quot;]*&quot;&#39;</span>',
    `'data-zfb-island="[^"]*"'`,
  ],
  ["&lt;div data-zfb-island=&quot;Demo&quot;&gt;", '<div data-zfb-island="Demo">'],
  ['&lt;div data-zfb-island-skip-ssr="Demo"&gt;', '<div data-zfb-island-skip-ssr="Demo">'],
  ["DATA-ZFB-ISLAND = x", "DATA-ZFB-ISLAND = x"],
  ["&lt;!--zr:1:9:h--&gt;&lt;!--/zr:1:9--&gt;", "<!--zr:1:9:h--><!--/zr:1:9-->"],
  [`<span title='data-zfb-island="Demo"'>x</span>`, "x"],
  [`<span title="data-zfb-island-skip-ssr=Demo">x</span>`, "x"],
  [`<span title="<!--zr:1:9:h-->">x</span>`, "x"],
  [`<span title=data-zfb-island=Demo>x</span>`, "x"],
  [`<span data-zfb-islands="Demo" data-zfb-island-x>x</span>`, "x"],
  ["<!-- data-zfb-island=x -->a", "a"],
  ["<!---->a<!-->b<!--->c", "abc"],
  ["a < b <", "a < b <"],
];

const rejected: [string, string][] = [
  ["double-quoted attribute", '<div data-zfb-island="Demo"></div>'],
  ["single-quoted attribute", "<div data-zfb-island='Demo'></div>"],
  ["unquoted attribute", "<div data-zfb-island=Demo></div>"],
  ["boolean attribute", "<div data-zfb-island></div>"],
  ["boolean attribute before another", '<div data-zfb-island class="x"></div>'],
  ["mixed case", '<div DATA-zfb-IsLaNd="Demo"></div>'],
  ["spaces around equals", '<div data-zfb-island = "Demo"></div>'],
  ["tab/newline/formfeed separators", '<div\tdata-zfb-island\n=\f"Demo"></div>'],
  ["CRLF separator", '<div\r\ndata-zfb-island="Demo"></div>'],
  ["skip-SSR variant", '<div data-zfb-island-skip-ssr="Demo"></div>'],
  ["skip-SSR boolean mixed case", "<div Data-Zfb-Island-Skip-SSR></div>"],
  ["slash separator", "<div/data-zfb-island=Demo></div>"],
  ["glued after quoted value", '<div title="a"data-zfb-island="Demo"></div>'],
  ["after a quote-bearing value", `<div title='"' data-zfb-island></div>`],
  ["after an unquoted value", "<div title=a data-zfb-island></div>"],
  ["end tag attribute", "</div data-zfb-island=Demo>"],
  ["unterminated tag with marker", '<div data-zfb-island="Demo"'],
  ["opening comment marker", "<!--zr:1:9:h--><b>reserved</b><!--/zr:1:9-->"],
  ["closing comment marker", "<!--/zr:1:1--><b>reserved</b>"],
  ["bogus bang comment marker", "<!zr:1:9:h>"],
  ["bogus closing comment marker", "<!/zr:1:9>"],
  ["bogus end-tag comment marker", "</%zr:1:9>"],
  ["processing-instruction comment marker", "<?zr:1:9:h>"],
  ["comment containing the marker", "<!-- zr:1:9:h -->"],
  ["unterminated tag", '<span title="x'],
  ["unterminated comment", "<!-- x"],
  ["trailing end-tag opener", "a</"],
  ["trailing bang", "a<!"],
  [
    "RCDATA end inside an attribute",
    '<textarea><i title="</textarea><i data-zfb-island>"></textarea>',
  ],
  ["raw-text ancestor end inside an attribute", '<i title="</noscript><i data-zfb-island=x>">'],
  ["script text", '<script>"data-zfb-island"</script>'],
  ["CDATA in foreign content", '<svg><![CDATA[><i title="]]><i data-zfb-island=1>">'],
];

describe("rawHtml reserved boundaries", () => {
  it.each(accepted)("renders display-only %j with exact text", (payload, text) => {
    expect(rawHtmlReserved(payload, "pre")).toBe(false);
    expect(renderToString(h("pre", { rawHtml: payload }))).toBe(`<pre>${payload}</pre>`);
    const template = document.createElement("template");
    template.innerHTML = payload;
    expect(template.content.textContent).toBe(text);
    const nodes = document.createTreeWalker(template.content, NodeFilter.SHOW_ALL);
    for (let node = nodes.nextNode(); node; node = nodes.nextNode()) {
      if (node instanceof Element) {
        expect(node.hasAttribute("data-zfb-island")).toBe(false);
        expect(node.hasAttribute("data-zfb-island-skip-ssr")).toBe(false);
      }
      if (node.nodeType === Node.COMMENT_NODE) expect(node.nodeValue).not.toMatch(/zr:1:/);
    }
  });

  it.each(rejected)("rejects a %s", (_, payload) => {
    expect(rawHtmlReserved(payload, "span")).toBe(true);
    expect(() => renderToString(h("div", { rawHtml: payload }))).toThrow(
      "ZR_RAW_HTML: reserved boundary in rawHtml",
    );
  });

  it("applies the textual rule to raw-text hosts", () => {
    expect(rawHtmlReserved("data-zfb-island=&quot;Demo&quot;", "pre")).toBe(false);
    for (const tag of ["noscript", "xmp", "noembed", "noframes", "plaintext"])
      expect(rawHtmlReserved("data-zfb-island=&quot;Demo&quot;", tag)).toBe(true);
  });

  it("keeps the documented script/style contract", () => {
    expect(
      renderToString(h("script", { rawHtml: "document.querySelector('[data-zfb-island]')" })),
    ).toBe("<script>document.querySelector('[data-zfb-island]')</script>");
    expect(() => renderToString(h("style", { rawHtml: "/* data-zfb-island= */" }))).toThrow(
      "ZR_RAW_HTML",
    );
  });
});

// #3625 combined payload: the inner HTML of the generated module that
// crates/zudo-wind/tests/extract_offsets.rs scans as a raw source.
describe("rawHtml combined #3569/#3570 payload", () => {
  const html =
    "<span class=\"line\">grep -roh 'data-zfb-island=&quot;[^&quot;]*&quot;' dist/ \u2014 \u65e5\u672c\u8a9e \u{1f389}</span>";

  it("renders the quoted text exactly with no island marker", () => {
    expect(rawHtmlReserved(html, "pre")).toBe(false);
    const markup = renderToString(h("pre", { rawHtml: html }));
    expect(markup).toBe(`<pre>${html}</pre>`);
    const host = document.createElement("div");
    host.innerHTML = markup;
    expect(host.querySelector("pre")?.textContent).toBe(
      `grep -roh 'data-zfb-island="[^"]*"' dist/ \u2014 \u65e5\u672c\u8a9e \u{1f389}`,
    );
    expect(host.querySelector("[data-zfb-island]")).toBeNull();
  });
});

describe.each(["hydrate", "mount"] as const)("%s rawHtml reserved boundaries", (mode) => {
  const attach = mode === "hydrate" ? hydrate : mount;
  const identity = { component: "Demo", build: "b1" };
  let diagnostics: Diagnostic[];
  const options = () => ({ identity, report: (item: Diagnostic) => diagnostics.push(item) });
  beforeEach(() => {
    document.body.replaceChildren();
    diagnostics = [];
  });
  const demo = (payload: string) =>
    function Demo() {
      return h("pre", { rawHtml: payload });
    };
  function server(payload: string): Element {
    const host = document.createElement("div");
    document.body.append(host);
    host.innerHTML = renderToString(islandRoot(h(demo(payload), {}), { identity }));
    return host.firstElementChild!;
  }

  it("accepts display-only marker text", () => {
    const payload = accepted[2]![0];
    const container = server(payload);
    expect(attach(h(demo(payload), {}), container, options())).not.toBeNull();
    expect(diagnostics).toEqual([]);
    expect(container.querySelector("pre")?.textContent).toBe(accepted[2]![1]);
    expect(container.querySelector("pre [data-zfb-island]")).toBeNull();
  });

  it("rejects a real marker", () => {
    const container = server("safe");
    expect(attach(h(demo("<i data-zfb-island>"), {}), container, options())).toBeNull();
    expect(diagnostics[0]?.code).toBe("ZR_RAW_HTML");
  });

  it("rejects a script payload that consumes its closing tag", () => {
    const scriptDemo = (payload: string) =>
      function Demo() {
        return h("script", { rawHtml: payload });
      };
    const host = document.createElement("div");
    document.body.append(host);
    host.innerHTML = renderToString(islandRoot(h(scriptDemo("safe"), {}), { identity }));
    const container = host.firstElementChild!;
    expect(attach(h(scriptDemo("<!--<script>"), {}), container, options())).toBeNull();
    expect(diagnostics[0]?.code).toBe("ZR_RAW_HTML");
  });
});
