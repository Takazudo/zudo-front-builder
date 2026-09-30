// @vitest-environment node
import { describe, expect, it } from "vitest";
import { Fragment, h, signal, type Child } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";

function islandPre(...children: Child[]): string {
  function Pre() {
    return h("pre", { children: children.length === 1 ? children[0] : children });
  }
  const html = renderToString(
    islandRoot(h(Pre, null), { identity: { component: "Pre", build: "b1" } }),
  );
  return html.match(/<pre>[\s\S]*?<\/pre>/)?.[0] ?? "";
}

describe("pre initial newline serialization", () => {
  it.each([
    ["no newline", "abc", "abc"],
    ["one LF", "\nabc", "\n\nabc"],
    ["two LFs", "\n\nabc", "\n\n\nabc"],
    ["CRLF", "\r\nabc", "\n\r\nabc"],
  ])("protects %s in static and island markup", (_name, source, emitted) => {
    expect(renderToString(h("pre", null, source))).toBe(`<pre>${emitted}</pre>`);
    expect(islandPre(source)).toBe(`<pre>${emitted}</pre>`);
  });

  it("decides from serialized siblings, including empty children", () => {
    expect(renderToString(h("pre", null, "", "\nabc"))).toBe("<pre>\n\nabc</pre>");
    expect(islandPre("", "\nabc")).toBe("<pre>\n\nabc</pre>");
    expect(renderToString(h("pre", null, "\n", signal("abc")))).toBe("<pre>\n\nabc</pre>");
    expect(islandPre("\n", signal("abc"))).toBe("<pre>\n\n<!--zr:1:1:t-->abc<!--/zr:1:1--></pre>");
  });

  it("does not compensate when an island marker precedes the text", () => {
    const value = signal("\nabc");
    expect(renderToString(h("pre", null, value))).toBe("<pre>\n\nabc</pre>");
    expect(islandPre(value)).toBe("<pre><!--zr:1:1:t-->\nabc<!--/zr:1:1--></pre>");
  });

  it("compensates flat arrays but respects nested array and Fragment markers", () => {
    const flat: Child[] = ["\nabc"];
    const nested: Child[] = [["\nabc"]];
    expect(renderToString(h("pre", null, flat))).toBe("<pre>\n\nabc</pre>");
    expect(renderToString(h("pre", null, nested))).toBe("<pre>\n\nabc</pre>");
    expect(islandPre(flat)).toBe("<pre>\n\nabc</pre>");
    expect(islandPre(nested)).toBe("<pre><!--zr:1:1:f-->\nabc<!--/zr:1:1--></pre>");
    const fragment = h(Fragment, null, "\nabc");
    expect(renderToString(h("pre", null, fragment))).toBe("<pre>\n\nabc</pre>");
    expect(islandPre(fragment)).toBe("<pre><!--zr:1:1:f-->\nabc<!--/zr:1:1--></pre>");
  });

  it("uses the leading component marker in islands", () => {
    function Text() {
      return "\nabc";
    }
    expect(renderToString(h("pre", null, h(Text, null)))).toBe("<pre>\n\nabc</pre>");
    expect(islandPre(h(Text, null))).toBe("<pre><!--zr:1:1:c-->\nabc<!--/zr:1:1--></pre>");
  });

  it("leaves nested code and opaque rawHtml unchanged", () => {
    const code = h("code", null, "\nabc");
    expect(renderToString(h("pre", null, code))).toBe("<pre><code>\nabc</code></pre>");
    expect(islandPre(code)).toBe("<pre><code>\nabc</code></pre>");
    expect(renderToString(h("pre", { rawHtml: "\nabc" }))).toBe("<pre>\nabc</pre>");
    function Raw() {
      return h("pre", { rawHtml: "\nabc" });
    }
    expect(
      renderToString(islandRoot(h(Raw, null), { identity: { component: "Raw", build: "b1" } })),
    ).toContain("<pre><!--zr:1:1:h-->\nabc<!--/zr:1:1--></pre>");
  });
});
