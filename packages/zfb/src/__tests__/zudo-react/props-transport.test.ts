// @vitest-environment node
import { describe, expect, it } from "vite-plus/test";
import { h, signal } from "../../zudo-react/index.js";
import { normalizeProps, parseProps, serializeProps } from "../../zudo-react/props-transport.js";

describe("strict props transport", () => {
  it("round trips punctuation and script terminators as JSON", () => {
    const input = { text: `</script> " & <`, nested: { children: [1, true, null] } };
    expect(parseProps(serializeProps(input))).toEqual(input);
  });
  it.each([
    [{ bad: [undefined] }, "ZR_PROPS_UNDEFINED", "props.bad[0]"],
    [{ bad: () => 1 }, "ZR_PROPS_FUNCTION", "props.bad"],
    [{ bad: Symbol() }, "ZR_PROPS_SYMBOL", "props.bad"],
    [{ bad: 1n }, "ZR_PROPS_BIGINT", "props.bad"],
    [{ bad: Infinity }, "ZR_PROPS_NUMBER", "props.bad"],
    [{ bad: NaN }, "ZR_PROPS_NUMBER", "props.bad"],
    [{ bad: -Infinity }, "ZR_PROPS_NUMBER", "props.bad"],
    [{ bad: new Date() }, "ZR_PROPS_OBJECT_KIND", "props.bad"],
    [{ bad: new Map() }, "ZR_PROPS_OBJECT_KIND", "props.bad"],
    [{ bad: new Set() }, "ZR_PROPS_OBJECT_KIND", "props.bad"],
    [{ bad: Promise.resolve(1) }, "ZR_PROPS_OBJECT_KIND", "props.bad"],
    [
      {
        bad: new (class Example {
          value = 1;
        })(),
      },
      "ZR_PROPS_OBJECT_KIND",
      "props.bad",
    ],
    [{ bad: h("div", null) }, "ZR_PROPS_RUNTIME_VALUE", "props.bad"],
    [{ bad: signal(1) }, "ZR_PROPS_RUNTIME_VALUE", "props.bad"],
    [{ bad: [1, , 3] }, "ZR_PROPS_UNDEFINED", "props.bad[1]"],
  ])("rejects invalid values with paths", (input, code, path) => {
    expect(() => serializeProps(input)).toThrow(`${code} at ${path}`);
  });
  it("rejects cycles, hidden properties, accessors and forbidden keys", () => {
    const cycle: Record<string, unknown> = {};
    cycle.self = cycle;
    expect(() => serializeProps(cycle)).toThrow(/ZR_PROPS_CYCLE at props.self.*props/);
    expect(() => serializeProps(Object.defineProperty({}, "secret", { value: 1 }))).toThrow(
      "ZR_PROPS_PROPERTY at props.secret",
    );
    expect(() =>
      serializeProps(
        Object.defineProperty({}, "x", {
          get() {
            throw Error("called");
          },
          enumerable: true,
        }),
      ),
    ).toThrow("ZR_PROPS_PROPERTY at props.x");
    expect(() => parseProps('{"nested":{"constructor":1}}')).toThrow(
      "ZR_PROPS_KEY at props.nested.constructor",
    );
    expect(() => parseProps("[]")).toThrow("ZR_PROPS_OBJECT_KIND at props");
    expect(() => serializeProps({ bad: { toJSON: () => 1 } })).toThrow(
      "ZR_PROPS_FUNCTION at props.bad.toJSON",
    );
    expect(() => serializeProps(Object.assign([], { extra: 1 }) as never)).toThrow(
      "ZR_PROPS_OBJECT_KIND at props",
    );
    expect(() => serializeProps({ bad: Object.assign([1], { extra: 2 }) })).toThrow(
      "ZR_PROPS_PROPERTY at props.bad[extra]",
    );
    const withSymbol = Object.assign({}, { [Symbol("secret")]: 1 });
    expect(() => serializeProps(withSymbol)).toThrow("ZR_PROPS_PROPERTY at props");
  });
  it("omits only empty top-level children", () => {
    expect(serializeProps({ a: 1, children: [null, false, []] })).toBe('{"a":1}');
    expect(() => serializeProps({ children: "lost" })).toThrow(
      "ZR_PROPS_CHILDREN at props.children",
    );
    expect(() => parseProps('{"children":null}')).toThrow("ZR_PROPS_CHILDREN at props.children");
  });
  it("preserves repeated references and null-prototype records", () => {
    const nested = Object.assign(Object.create(null), { value: -0 });
    expect(serializeProps({ first: nested, second: nested })).toBe(
      '{"first":{"value":0},"second":{"value":0}}',
    );
    const normalized = normalizeProps({ first: nested, second: nested });
    expect(Object.getPrototypeOf(normalized.first)).toBeNull();
    expect(normalized.first).not.toBe(normalized.second);
    expect(normalized.first).not.toBe(nested);
  });
  it("omits undefined record members deeply without changing the input", () => {
    expect(serializeProps({ missing: undefined })).toBe("{}");
    const nested = Object.defineProperty({ missing: undefined, present: null }, "fixed", {
      value: { absent: undefined, retained: 1 },
      enumerable: true,
      configurable: false,
      writable: false,
    });
    const original = Object.getOwnPropertyDescriptors(nested);
    const input = { nested, items: [{ absent: undefined, present: null }] };
    const normalized = normalizeProps(input);
    expect(normalized).toEqual({
      nested: { present: null, fixed: { retained: 1 } },
      items: [{ present: null }],
    });
    expect(serializeProps(input)).toBe(
      '{"nested":{"present":null,"fixed":{"retained":1}},"items":[{"present":null}]}',
    );
    expect(Object.getOwnPropertyDescriptors(nested)).toEqual(original);
    expect(nested.missing).toBeUndefined();
    expect(Object.hasOwn(nested, "missing")).toBe(true);
    expect(normalized.nested).not.toBe(nested);
  });
  it("round trips omitted record keys separately from explicit null keys", () => {
    const input = {
      omitted: undefined,
      explicitNull: null,
      records: [{ description: undefined }, { description: null }],
    };
    const normalized = normalizeProps(input);
    const parsed = parseProps(serializeProps(input));

    for (const props of [normalized, parsed]) {
      expect(Object.hasOwn(props, "omitted")).toBe(false);
      expect(Object.hasOwn(props, "explicitNull")).toBe(true);
      expect(props.explicitNull).toBeNull();
      expect(
        Object.hasOwn((props.records as Array<Record<string, unknown>>)[0]!, "description"),
      ).toBe(false);
      expect(
        Object.hasOwn((props.records as Array<Record<string, unknown>>)[1]!, "description"),
      ).toBe(true);
      expect((props.records as Array<Record<string, unknown>>)[1]!.description).toBeNull();
    }
    expect(Object.hasOwn(input, "omitted")).toBe(true);
    expect(Object.hasOwn(input.records[0]!, "description")).toBe(true);
  });
  it.each([
    [
      "array undefined",
      () => ({ omitted: undefined, bad: [undefined] }),
      "ZR_PROPS_UNDEFINED at props.bad[0]",
    ],
    [
      "array hole",
      () => ({ omitted: undefined, bad: Array(1) }),
      "ZR_PROPS_UNDEFINED at props.bad[0]",
    ],
    ["function", () => ({ omitted: undefined, bad: () => 1 }), "ZR_PROPS_FUNCTION at props.bad"],
    ["symbol", () => ({ omitted: undefined, bad: Symbol("bad") }), "ZR_PROPS_SYMBOL at props.bad"],
    ["bigint", () => ({ omitted: undefined, bad: 1n }), "ZR_PROPS_BIGINT at props.bad"],
    ["non-finite number", () => ({ omitted: undefined, bad: NaN }), "ZR_PROPS_NUMBER at props.bad"],
    [
      "cycle",
      () => {
        const bad: Record<string, unknown> = {};
        bad.self = bad;
        return { omitted: undefined, bad };
      },
      "ZR_PROPS_CYCLE at props.bad.self",
    ],
    [
      "forbidden key",
      () => ({
        omitted: undefined,
        ...Object.defineProperty({}, "constructor", { value: undefined, enumerable: true }),
      }),
      "ZR_PROPS_KEY at props.constructor",
    ],
  ] as const)(
    "still rejects %s when an undefined record member can be omitted",
    (_label, makeProps, diagnostic) => {
      expect(() => normalizeProps(makeProps())).toThrow(diagnostic);
      expect(() => serializeProps(makeProps())).toThrow(diagnostic);
    },
  );
  it("rejects top-level undefined and does not read empty children accessors", () => {
    expect(() => serializeProps(undefined as never)).toThrow("ZR_PROPS_OBJECT_KIND at props");
    let called = false;
    const children = Object.defineProperty([], "0", {
      get() {
        called = true;
        return null;
      },
      enumerable: true,
    });
    expect(() => serializeProps({ children })).toThrow("ZR_PROPS_CHILDREN at props.children");
    expect(called).toBe(false);
  });
});
