// @vitest-environment node
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Fragment, getScope, h, signal, flush } from "../../zudo-react/index.js";
import { subscriberCount } from "../../zudo-react/reactive.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import * as server from "../../zudo-react/server.js";

const packageJson = JSON.parse(
  readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), "../../../package.json"), "utf8"),
) as {
  exports: Record<string, { types: string; default: string }>;
  publishConfig: { exports: Record<string, { types: string; default: string }> };
};

const identity = { component: "Demo", build: "b1" };
function Demo() {
  return h("p", null, signal("A"), signal("B"));
}

describe("server renderer", () => {
  it("renders islands from the same normalized nested props sent to the client", () => {
    const input = {
      missing: undefined,
      nested: { missing: undefined, present: null },
      records: [
        {
          description: undefined,
          details: [{ description: undefined }, { description: null }],
        },
        { description: null, details: [] },
      ],
    };
    function OwnKeys(props: typeof input) {
      return h(
        "p",
        null,
        JSON.stringify({
          missing: Object.hasOwn(props, "missing"),
          nested: {
            missing: Object.hasOwn(props.nested, "missing"),
            present: Object.hasOwn(props.nested, "present"),
            value: props.nested.present,
          },
          records: props.records.map((record) => ({
            description: Object.hasOwn(record, "description"),
            value: Object.hasOwn(record, "description") ? record.description : "omitted",
            details: record.details.map((detail) => ({
              description: Object.hasOwn(detail, "description"),
              value: Object.hasOwn(detail, "description") ? detail.description : "omitted",
            })),
          })),
        }),
      );
    }
    const html = renderToString(
      islandRoot(h(OwnKeys, input), { identity: { component: "OwnKeys", build: "b1" } }),
    );
    expect(html).toContain(
      'data-props="{&quot;nested&quot;:{&quot;present&quot;:null},&quot;records&quot;:[{&quot;details&quot;:[{},{&quot;description&quot;:null}]},{&quot;description&quot;:null,&quot;details&quot;:[]}]}',
    );
    expect(html).toContain(
      '{"missing":false,"nested":{"missing":false,"present":true,"value":null},"records":[{"description":false,"value":"omitted","details":[{"description":false,"value":"omitted"},{"description":true,"value":null}]},{"description":true,"value":null,"details":[]}]}',
    );
    expect(Object.hasOwn(input, "missing")).toBe(true);
    expect(Object.hasOwn(input.nested, "missing")).toBe(true);
    expect(Object.hasOwn(input.records[0]!, "description")).toBe(true);
  });
  it("exports exactly the locked server entry values", () => {
    expect(Object.keys(server).sort()).toEqual(["islandRoot", "renderToString", "serializeProps"]);
    expect(typeof globalThis.document).toBe("undefined");
  });

  it("keeps source and published zudo-react export maps aligned", () => {
    const entries = {
      "./zudo-react": [
        "./src/zudo-react/index.ts",
        "./dist/zudo-react/index.d.ts",
        "./dist/zudo-react/index.js",
      ],
      "./zudo-react/jsx-runtime": [
        "./src/zudo-react/jsx-runtime.ts",
        "./dist/zudo-react/jsx-runtime.d.ts",
        "./dist/zudo-react/jsx-runtime.js",
      ],
      "./zudo-react/jsx-dev-runtime": [
        "./src/zudo-react/jsx-dev-runtime.ts",
        "./dist/zudo-react/jsx-dev-runtime.d.ts",
        "./dist/zudo-react/jsx-dev-runtime.js",
      ],
      "./zudo-react/server": [
        "./src/zudo-react/server.ts",
        "./dist/zudo-react/server.d.ts",
        "./dist/zudo-react/server.js",
      ],
      "./zudo-react/client": [
        "./src/zudo-react/client.ts",
        "./dist/zudo-react/client.d.ts",
        "./dist/zudo-react/client.js",
      ],
      "./zudo-react/testing": [
        "./src/zudo-react/testing.ts",
        "./dist/zudo-react/testing.d.ts",
        "./dist/zudo-react/testing.js",
      ],
    } as const;
    const expectedSubpaths = Object.keys(entries).sort();
    const zudoReactSubpaths = (exports: Record<string, unknown>) =>
      Object.keys(exports)
        .filter((subpath) => /^\.\/zudo-react(?:\/.*)?$/.test(subpath))
        .sort();

    expect(zudoReactSubpaths(packageJson.exports)).toEqual(expectedSubpaths);
    expect(zudoReactSubpaths(packageJson.publishConfig.exports)).toEqual(expectedSubpaths);

    for (const [subpath, [source, types, runtime]] of Object.entries(entries)) {
      expect(packageJson.exports[subpath]).toEqual({ types: source, default: source });
      expect(packageJson.publishConfig.exports[subpath]).toEqual({
        types,
        default: runtime,
      });
    }
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
  it("renders a complete page head and SVG with standard markup attributes", () => {
    const page = h(
      "html",
      null,
      h(
        "head",
        null,
        h("meta", { property: "og:title", content: "Standard markup" }),
        h("link", {
          rel: "preload",
          href: "/assets/body.woff2",
          as: "font",
          integrity: "sha256-x",
        }),
        h("script", { src: "/assets/site.js", defer: true, nonce: "page-nonce" }),
      ),
      h(
        "body",
        null,
        h(
          "svg",
          { xmlns: "http://www.w3.org/2000/svg", viewBox: "0 0 16 16" },
          h("use", { href: "#shape", "xlink:href": "#shape" }),
        ),
      ),
    );

    expect(renderToString(page)).toBe(
      '<html><head><meta property="og:title" content="Standard markup"><link rel="preload" href="/assets/body.woff2" as="font" integrity="sha256-x"><script src="/assets/site.js" defer nonce="page-nonce"></script></head><body><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><use href="#shape" xlink:href="#shape"></use></svg></body></html>',
    );
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
          style: { "font-size": "12px", "--accent": "red" },
          "aria-pressed": false,
          hidden: true,
        }),
      ),
    ).toBe('<div style="font-size:12px;--accent:red;" aria-pressed="false" hidden></div>');
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
