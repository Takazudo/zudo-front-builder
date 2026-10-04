// @vitest-environment node
import { describe, expect, it } from "vite-plus/test";
import { computed, h, signal } from "../../zudo-react/index.js";
import { renderToString } from "../../zudo-react/server.js";
import { rawHtmlReserved } from "../../zudo-react/raw-html.js";

const scriptBoundaryCases = [
  ["plain JavaScript string", `window.template = "<script>";`, true],
  ["ordinary script-looking text", `<script>`, true],
  ["comment opener", `<!--`, true],
  ["closed escape", `<!-- <script> -->`, true],
  ["escape with a near-miss name", `<!--<scripture>`, true],
  ["escape with a non-ASCII name", `<!--<scrıpt>`, true],
  ["escape with a script name and space", `<!--<ScRiPt >`, false],
  ["escape with a script name and tab", `<!--<SCRIPT\t>`, false],
  ["escape with a script name and LF", `<!--<script\n>`, false],
  ["escape with a script name and form feed", `<!--<script\f>`, false],
  ["escape with a script name and slash", `<!--<script/>`, false],
  ["escape with a script name and CR", `<!--<script\r>`, false],
  ["escape with a script name and equals", `<!--<script=>`, true],
  ["double escape exits then reenters", `<!--<script></script><script>`, false],
  ["double escape dash-dash exits at greater-than", `<!--<script>-->`, true],
  ["incomplete escape start", `<!`, true],
  ["incomplete double escape name", `<!--<script`, true],
  ["incomplete double escape exit", `<!--<script></scr`, false],
  ["incomplete near-miss exit", `<!--<script></scripture`, false],
  ["ordinary JavaScript comment syntax", `const x = "<!--";`, true],
] as const;

describe("script rawHtml closing boundary", () => {
  it.each(scriptBoundaryCases)("%s", (_, payload, accepted) => {
    expect(rawHtmlReserved(payload, "script")).toBe(!accepted);
    const render = () => renderToString(h("script", { rawHtml: payload }));
    if (accepted) expect(render()).toBe(`<script>${payload}</script>`);
    else expect(render).toThrow("ZR_RAW_HTML: reserved boundary in rawHtml");
    // Style is RAWTEXT: script's comment and double-escape states do not apply.
    expect(rawHtmlReserved(payload, "style")).toBe(false);
  });

  it("keeps the existing closing-tag rejection even when double escape would exit", () => {
    const payload = "<!--<script></SCRIPT>";
    expect(rawHtmlReserved(payload, "script")).toBe(false);
    expect(() => renderToString(h("script", { rawHtml: payload }))).toThrow(
      "ZR_RAW_HTML: closing script in rawHtml",
    );
  });
});

describe.each([
  ["script", `window.label = "A & B <tag>";`],
  ["style", `body::before { content: "A & B <tag>"; }`],
] as const)("raw-text <%s>", (tag, source) => {
  it("renders trusted rawHtml verbatim, including quotes, ampersands, and less-than signs", () => {
    expect(renderToString(h(tag, { rawHtml: source }))).toBe(`<${tag}>${source}</${tag}>`);
  });

  it("allows empty rawHtml and childless tags, including external scripts", () => {
    expect(renderToString(h(tag, { rawHtml: "" }))).toBe(`<${tag}></${tag}>`);
    expect(renderToString(h(tag, null))).toBe(`<${tag}></${tag}>`);
    if (tag === "script") {
      expect(renderToString(h("script", { src: "/external.js" }))).toBe(
        '<script src="/external.js"></script>',
      );
    }
  });

  it.each([
    tag === "script" ? "</ScRiPt>" : "</StYlE>",
    "<!--zr:1:9:h-->",
    "<!--/zr:1:9-->",
    '<div data-zfb-island="Nested"></div>',
    '<div data-zfb-island-skip-ssr="Nested"></div>',
  ])("rejects unsafe rawHtml %j", (rawHtml) => {
    expect(() => renderToString(h(tag, { rawHtml }))).toThrow("ZR_RAW_HTML");
  });

  it("rejects children and reactive or non-string rawHtml at runtime", () => {
    expect(() => renderToString(h(tag, { children: "child" } as never))).toThrow("ZR_RAW_HTML");
    const rawHtmlWithChildren = { rawHtml: source, children: "child" };
    expect(() => renderToString(h(tag, { ...rawHtmlWithChildren } as never))).toThrow(
      "ZR_RAW_HTML",
    );
    expect(() => renderToString(h(tag, { rawHtml: signal(source) } as never))).toThrow(
      "ZR_RAW_HTML",
    );
    expect(() => renderToString(h(tag, { rawHtml: computed(() => source) } as never))).toThrow(
      "ZR_RAW_HTML",
    );
    expect(() => renderToString(h(tag, { rawHtml: null } as never))).toThrow("ZR_RAW_HTML");
  });
});

describe("non-raw-text JSX contracts", () => {
  it("keeps title and textarea children rendered as ordinary text", () => {
    expect(renderToString(h("title", null, "A & B"))).toBe("<title>A &amp; B</title>");
    expect(renderToString(h("textarea", null, "A & B"))).toBe("<textarea>A &amp; B</textarea>");
  });

  it("keeps reactive rawHtml available on ordinary supported containers", () => {
    expect(renderToString(h("div", { rawHtml: signal("<b>A & B</b>") }))).toBe(
      "<div><b>A & B</b></div>",
    );
  });
});
