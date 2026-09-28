// @vitest-environment node
import { describe, expect, it } from "vitest";
import { h, signal, computed } from "../../zudo-react/index.js";
import { renderToString } from "../../zudo-react/server.js";

describe("server form representation", () => {
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
