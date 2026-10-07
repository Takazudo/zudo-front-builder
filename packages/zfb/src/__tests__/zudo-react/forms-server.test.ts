// @vitest-environment node
import { describe, expect, it } from "vite-plus/test";
import { h, signal, computed } from "../../zudo-react/index.js";
import { renderToString } from "../../zudo-react/server.js";

describe("server form representation", () => {
  it("serializes display-only values without markers or coercion", () => {
    expect(renderToString(h("input", { readonly: true, value: signal('a&"b') }))).toBe(
      '<input readonly value="a&amp;&quot;b">',
    );
    expect(
      renderToString(h("input", { type: undefined, readonly: true, value: signal("a") })),
    ).toBe('<input readonly value="a">');
    expect(renderToString(h("textarea", { disabled: true, value: signal("\n<a&b") }))).toBe(
      "<textarea disabled>\n\n&lt;a&amp;b</textarea>",
    );
    expect(() => renderToString(h("input", { readonly: true, value: signal(1) }))).toThrow(
      "ZR_MODEL_VALUE",
    );
    expect(() => renderToString(h("textarea", { disabled: true, value: signal(1) }))).toThrow(
      "ZR_MODEL_VALUE",
    );
  });
  it("rejects display-only value conflicts", () => {
    for (const props of [
      { readonly: true, value: signal("a"), defaultValue: "b" },
      { readonly: true, value: signal("a"), modelValue: signal("b") },
      { readonly: true, value: signal("a"), children: "b" },
    ])
      expect(() => renderToString(h("textarea", props))).toThrow("ZR_MODEL_CONFLICT");
  });
  it.each([
    [
      "text",
      h("input", { defaultValue: "a&b" }),
      h("input", { modelValue: signal("a&b") }),
      '<input value="a&amp;b">',
    ],
    [
      "textarea",
      h("textarea", { defaultValue: "\nabc" }),
      h("textarea", { modelValue: signal("\nabc") }),
      "<textarea>\n\nabc</textarea>",
    ],
    [
      "checkbox",
      h("input", { type: "checkbox", defaultChecked: true }),
      h("input", { type: "checkbox", modelChecked: signal(true) }),
      '<input type="checkbox" checked>',
    ],
    [
      "select",
      h("select", {
        defaultValue: "b",
        children: [
          h("option", { value: "a", children: "A" }),
          h("option", { value: "b", children: "B" }),
        ],
      }),
      h("select", {
        modelValue: signal("b"),
        children: [
          h("option", { value: "a", children: "A" }),
          h("option", { value: "b", children: "B" }),
        ],
      }),
      '<select><option value="a">A</option><option value="b" selected>B</option></select>',
    ],
    [
      "radio",
      h("input", { type: "radio", name: "r", value: "b", defaultChecked: true }),
      h("input", { type: "radio", name: "r", value: "b", modelValue: signal<string | null>("b") }),
      '<input type="radio" name="r" value="b" checked>',
    ],
  ])("serializes %s defaults and models", (_name, fallback, model, output) => {
    expect(renderToString(fallback)).toBe(output);
    expect(renderToString(model)).toBe(output);
    expect(renderToString(model)).not.toMatch(
      /modelValue|modelChecked|defaultValue|defaultChecked/,
    );
  });
  it("rejects readonly, conflicts and missing select values", () => {
    expect(() => renderToString(h("input", { modelValue: computed(() => "x") }))).toThrow(
      "ZR_MODEL_READONLY",
    );
    expect(() => renderToString(h("input", { value: "x", defaultValue: "y" }))).toThrow(
      "ZR_MODEL_CONFLICT",
    );
    expect(() =>
      renderToString(
        h("select", {
          modelValue: signal("missing"),
          children: h("option", { value: "a", children: "A" }),
        }),
      ),
    ).toThrow("ZR_MODEL_VALUE");
    expect(() => renderToString(h("input", { type: "file", modelValue: signal("x") }))).toThrow(
      "ZR_MODEL_UNSUPPORTED",
    );
  });
});

describe("form diagnostics", () => {
  it.each(["color", "number", "range", "date", "time", "week", "month", "datetime-local"])(
    "rejects defaultValue on %s while accepting a static value",
    (type) => {
      expect(() => renderToString(h("input", { type, defaultValue: "2" }))).toThrow(
        "ZR_MODEL_UNSUPPORTED",
      );
      expect(renderToString(h("input", { type, value: "2" }))).toBe(
        `<input type="${type}" value="2">`,
      );
    },
  );
  it("rejects defaultValue on file inputs", () => {
    expect(() => renderToString(h("input", { type: "file", defaultValue: "x" }))).toThrow(
      "ZR_MODEL_UNSUPPORTED",
    );
  });
  it("rejects defaults on the opposite input kinds", () => {
    expect(() => renderToString(h("input", { defaultChecked: true }))).toThrow(
      "ZR_MODEL_UNSUPPORTED",
    );
    expect(() => renderToString(h("input", { type: "checkbox", defaultValue: "x" }))).toThrow(
      "ZR_MODEL_UNSUPPORTED",
    );
    expect(() =>
      renderToString(h("input", { type: "radio", name: "r", value: "x", defaultValue: "x" })),
    ).toThrow("ZR_MODEL_CONFLICT");
  });
  it("renders an unmodelled radio default without a name or value", () => {
    expect(renderToString(h("input", { type: "radio", defaultChecked: true }))).toBe(
      '<input type="radio" checked>',
    );
  });
  it.each([
    [
      "model/default",
      h("input", { modelValue: signal("a"), defaultValue: "b" }),
      "ZR_MODEL_CONFLICT",
    ],
    [
      "checked/default",
      h("input", { type: "checkbox", checked: true, defaultChecked: false }),
      "ZR_MODEL_CONFLICT",
    ],
    ["model/static", h("input", { modelValue: signal("a"), value: "a" }), "ZR_MODEL_CONFLICT"],
    [
      "contenteditable",
      h("div", { contenteditable: "true", modelValue: signal("a") }),
      "ZR_MODEL_UNSUPPORTED",
    ],
    [
      "multiple",
      h("select", {
        multiple: true,
        modelValue: signal("a"),
        children: h("option", { value: "a", children: "A" }),
      }),
      "ZR_MODEL_UNSUPPORTED",
    ],
    [
      "radio value",
      h("input", { type: "radio", name: "r", modelValue: signal<string | null>("a") }),
      "ZR_MODEL_UNSUPPORTED",
    ],
    [
      "radio no match",
      h("input", { type: "radio", name: "r", value: "a", modelValue: signal<string | null>("b") }),
      "ZR_MODEL_VALUE",
    ],
    [
      "duplicate options",
      h("select", {
        modelValue: signal("a"),
        children: [
          h("option", { value: "a", children: "A" }),
          h("option", { value: "a", children: "Again" }),
        ],
      }),
      "ZR_MODEL_CONFLICT",
    ],
  ])("rejects %s", (_name, node, code) => {
    expect(() => renderToString(node)).toThrow(code);
  });
});

describe("radio form association", () => {
  it("allows the same radio name in distinct native forms", () => {
    const left = signal<string | null>("a");
    const right = signal<string | null>("b");
    const page = h("div", {
      children: [
        h("form", {
          children: h("input", { type: "radio", name: "choice", value: "a", modelValue: left }),
        }),
        h("form", {
          children: h("input", { type: "radio", name: "choice", value: "b", modelValue: right }),
        }),
      ],
    });
    expect(renderToString(page)).toContain('<input type="radio" name="choice" value="b" checked>');
  });
});

describe("writable model brand", () => {
  it.each([
    h("input", { modelValue: computed(() => "x") }),
    h("textarea", { modelValue: computed(() => "x") }),
    h("input", { type: "checkbox", modelChecked: computed(() => true) }),
    h("select", {
      modelValue: computed(() => "x"),
      children: h("option", { value: "x", children: "X" }),
    }),
    h("input", {
      type: "radio",
      name: "r",
      value: "x",
      modelValue: computed<string | null>(() => "x"),
    }),
  ])("rejects computed models in every adapter", (node) => {
    expect(() => renderToString(node)).toThrow("ZR_MODEL_READONLY");
  });
  it("rejects a reactive object with a setter but no writable brand", () => {
    const fake = {
      $$zudoReactive: "zudo-react.reactive.v1",
      get value() {
        return "x";
      },
      set value(_next: string) {},
    };
    expect(() => renderToString(h("input", { modelValue: fake }))).toThrow("ZR_MODEL_READONLY");
  });
});
