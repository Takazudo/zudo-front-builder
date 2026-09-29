// @vitest-environment node
import { describe, expect, it } from "vitest";
import { Fragment, getScope, h, signal, flush } from "../../zudo-react/index.js";
import { subscriberCount } from "../../zudo-react/reactive.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import * as server from "../../zudo-react/server.js";

const identity = { component: "Demo", build: "b1" };
function Demo() {
  return h("p", null, signal("A"), signal("B"));
}

describe("server renderer", () => {
  it("exports exactly the locked server entry values", () => {
    expect(Object.keys(server).sort()).toEqual(["islandRoot", "renderToString", "serializeProps"]);
  });

  it("renders a whole document with exact bytes and no static markers", () => {
    const page = h(
      "html",
      null,
      h(
        "head",
        null,
        h("meta", { charset: "utf-8" }),
        h("title", null, "A & B"),
        h("template", { "data-x": "&" }),
      ),
      h("body", null, h("strong", null, "node-free"), h("a", { href: "./other" }, "link")),
    );
    const expected =
      '<html><head><meta charset="utf-8"><title>A &amp; B</title><template data-x="&amp;"></template></head><body><strong>node-free</strong><a href="./other">link</a></body></html>';
    expect(renderToString(page)).toBe(expected);
    expect(renderToString(h(Fragment, null, page))).toBe(expected);
    expect(renderToString(page)).toBe(expected);
  });
  it("renders SVG dimensions as strings or numbers and rejects booleans", () => {
    expect(renderToString(h("svg", { width: "16", height: 24 }))).toBe(
      '<svg width="16" height="24"></svg>',
    );
    expect(renderToString(h("svg", { width: signal(16), height: signal("24") }))).toBe(
      '<svg width="16" height="24"></svg>',
    );
    expect(() => renderToString(h("svg", { width: true }))).toThrow("ZR_ATTRIBUTE");
    expect(() => renderToString(h("svg", { height: false }))).toThrow("ZR_ATTRIBUTE");
  });
  it("allows ordinary content in table cells while keeping table structure intrinsic", () => {
    function CellContent() {
      return h("strong", null, "showcase");
    }
    const table = h(
      "table",
      null,
      h("caption", null, h(CellContent, null)),
      h(
        "tbody",
        null,
        h("tr", null, h("th", null, h(CellContent, null)), h("td", null, h(CellContent, null))),
      ),
    );
    expect(renderToString(table)).toBe(
      "<table><caption><strong>showcase</strong></caption><tbody><tr><th><strong>showcase</strong></th><td><strong>showcase</strong></td></tr></tbody></table>",
    );
    expect(() => renderToString(h("table", null, h("tr", null, h("td", null, "bad"))))).toThrow(
      "ZR_PARSER_CONTEXT",
    );
    expect(() =>
      renderToString(h("table", null, h("tbody", null, h("tr", null, h("div", null))))),
    ).toThrow("ZR_PARSER_CONTEXT");
  });
  it("renders deterministic local island regions", () => {
    const node = islandRoot(h(Demo, null), { identity });
    const expected =
      '<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><p><!--zr:1:1:t-->A<!--/zr:1:1--><!--zr:1:2:t-->B<!--/zr:1:2--></p><!--/zr:1:0--></div>';
    expect(renderToString(node)).toBe(expected);
    expect(renderToString(h("div", null, "other"))).toBe("<div>other</div>");
    expect(renderToString(node)).toBe(expected);
    expect(renderToString(h(Demo, null), { island: identity })).toBe(
      "<!--zr:1:0:c--><p><!--zr:1:1:t-->A<!--/zr:1:1--><!--zr:1:2:t-->B<!--/zr:1:2--></p><!--/zr:1:0-->",
    );
  });
  it("disposes setup after success and failure without activation", async () => {
    const value = signal(1);
    let activated = 0;
    function Read() {
      const scope = getScope();
      scope.onActivate(() => {
        activated++;
      });
      scope.effect(() => {
        activated++;
        void value.value;
      });
      return h("b", null, value);
    }
    expect(renderToString(h(Read, null))).toBe("<b>1</b>");
    expect(subscriberCount(value)).toBe(0);
    function Throw() {
      getScope().effect(() => {
        activated++;
      });
      throw Error("boom");
    }
    expect(() => renderToString(h(Throw, null))).toThrow("boom");
    await flush();
    expect(activated).toBe(0);
    expect(subscriberCount(value)).toBe(0);
  });
  it("renders form models and rejects invalid child values", () => {
    expect(renderToString(h("input", { modelValue: signal("x") }))).toBe('<input value="x">');
    expect(renderToString(h("input", { defaultValue: "x" }))).toBe('<input value="x">');
    expect(() => renderToString(h("div", null, {} as never))).toThrow("ZR_CHILD");
  });
  it("renders raw HTML and styles while enforcing parser contexts", () => {
    function Raw() {
      return h("section", { rawHtml: "<em>trusted</em>" });
    }
    const output = renderToString(
      islandRoot(h(Raw, null), { identity: { component: "Raw", build: "b1" } }),
    );
    expect(output).toContain("<section><!--zr:1:1:h--><em>trusted</em><!--/zr:1:1--></section>");
    expect(
      renderToString(
        h("div", {
          style: { "font-size": 12, "--accent": "red" },
          "aria-pressed": false,
          hidden: true,
        }),
      ),
    ).toBe('<div style="font-size:12;--accent:red;" aria-pressed="false" hidden></div>');
    expect(() => renderToString(h("script", { rawHtml: "</script>" }))).toThrow("ZR_RAW_HTML");
    expect(() => renderToString(h("table", null, h("tr", null)))).toThrow("ZR_PARSER_CONTEXT");
    expect(() => renderToString(h("svg", { rawHtml: "<path/>" }))).toThrow("ZR_RAW_HTML");
    expect(renderToString(h("svg", null, h("foreignObject", null, h("div", null, "ok"))))).toBe(
      "<svg><foreignObject><div>ok</div></foreignObject></svg>",
    );
  });
  it("validates island identity and skip-SSR fallback", () => {
    let calls = 0;
    function Fallback() {
      calls++;
      return h("b", null, "fallback");
    }
    const skipped = islandRoot(h(Demo, null), {
      identity,
      skipSsr: true,
      fallback: h(Fallback, null),
    });
    expect(renderToString(skipped)).toContain("<b>fallback</b></div>");
    expect(calls).toBe(1);
    expect(() =>
      renderToString(islandRoot(h(Demo, null), { identity: { component: "Wrong", build: "b1" } })),
    ).toThrow("ZR_ISLAND_IDENTITY");
    expect(() =>
      renderToString(islandRoot(h(Demo, null), { identity, skipSsr: true, fallback: skipped })),
    ).toThrow("ZR_NESTED_ISLAND");
  });
});
