// @vitest-environment node
import { describe, expect, it } from "vite-plus/test";
import { For, Show, h, signal } from "../../zudo-react/index.js";
import { renderToString } from "../../zudo-react/server.js";

const identity = { component: "Demo", build: "b1" };

describe("structure server output", () => {
  it("recognizes Show and For from another bundled module copy", () => {
    function OtherShow(): never {
      throw new Error("other Show must not be called as a component");
    }
    function OtherFor(): never {
      throw new Error("other For must not be called as a component");
    }
    Object.defineProperty(OtherShow, Symbol.for("@takazudo/zfb/zudo-react/Show"), {
      value: true,
    });
    Object.defineProperty(OtherFor, Symbol.for("@takazudo/zfb/zudo-react/For"), {
      value: true,
    });
    const when = signal(true);
    const each = signal(["one", "two"]);
    const node = h(
      "div",
      null,
      h(OtherShow, { when, children: () => h("b", null, "yes") }),
      h(OtherFor, {
        each,
        by: (item: string) => item,
        children: (item: { value: string }) => h("i", null, item.value),
      }),
    );
    expect(renderToString(node)).toBe("<div><b>yes</b><i>one</i><i>two</i></div>");
  });
  it("renders both conditional states with exact regions", () => {
    const when = signal(true);
    function Demo() {
      return Show({ when, children: () => h("b", null, "Yes") });
    }
    expect(renderToString(h(Demo, null), { island: identity })).toBe(
      "<!--zr:1:0:c--><!--zr:1:1:s:1--><b>Yes</b><!--/zr:1:1--><!--/zr:1:0-->",
    );
    expect(renderToString(h(Demo, null))).toBe("<b>Yes</b>");
    when.value = false;
    expect(renderToString(h(Demo, null), { island: identity })).toBe(
      "<!--zr:1:0:c--><!--zr:1:1:s:0--><!--/zr:1:1--><!--/zr:1:0-->",
    );
    expect(renderToString(h(Demo, null))).toBe("");
  });
  it("renders keyed and empty lists with exact regions", () => {
    const each = signal([{ id: "a" }, { id: "b" }, { id: "c" }]);
    function Demo() {
      return h(
        "ul",
        null,
        For({ each, by: (item) => item.id, children: (item) => h("li", null, item.value.id) }),
      );
    }
    expect(renderToString(h(Demo, null), { island: identity })).toBe(
      "<!--zr:1:0:c--><ul><!--zr:1:1:l--><!--zr:1:2:i:s61--><li>a</li><!--/zr:1:2--><!--zr:1:3:i:s62--><li>b</li><!--/zr:1:3--><!--zr:1:4:i:s63--><li>c</li><!--/zr:1:4--><!--/zr:1:1--></ul><!--/zr:1:0-->",
    );
    expect(renderToString(h(Demo, null))).toBe("<ul><li>a</li><li>b</li><li>c</li></ul>");
    each.value = [];
    expect(renderToString(h(Demo, null), { island: identity })).toBe(
      "<!--zr:1:0:c--><ul><!--zr:1:1:l--><!--/zr:1:1--></ul><!--/zr:1:0-->",
    );
  });
  it("rejects duplicate keys and parser-sensitive positions", () => {
    const each = signal([{ id: 1 }, { id: 1 }]);
    expect(() => renderToString(For({ each, by: (item) => item.id, children: () => "x" }))).toThrow(
      /ZR_DUPLICATE_KEY.*For.*1/,
    );
    expect(() =>
      renderToString(For({ each: signal([1]), by: () => true as never, children: () => "x" })),
    ).toThrow(/ZR_KEY_TYPE.*For.*true/);
    const when = signal(true);
    expect(() =>
      renderToString(h("select", null, Show({ when, children: () => h("option", null, "x") }))),
    ).toThrow(/ZR_PARSER_CONTEXT.*select/);
  });
});
