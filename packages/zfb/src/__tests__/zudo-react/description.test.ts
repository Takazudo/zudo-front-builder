import { describe, expect, it } from "vitest";
import { Fragment, flattenChildren, h, isDescription } from "../../zudo-react/index.js";
import { jsx, jsxs } from "../../zudo-react/jsx-runtime.js";
import { jsxDEV } from "../../zudo-react/jsx-dev-runtime.js";

describe("zudo-react descriptions", () => {
  it("copies props, normalizes keys and stays inert", () => {
    const props = { key: "in-props", children: "hello", id: "a" };
    const description = jsx("p", props, 4);
    expect(description).toEqual({
      $$zudo: "zudo-react.description.v1",
      type: "p",
      props: { children: "hello", id: "a" },
      key: 4,
    });
    expect(props.key).toBe("in-props");
    expect(Object.keys(description)).toContain("$$zudo");
    expect(isDescription({ ...description, props: { ...description.props } })).toBe(true);
    expect(jsxs(Fragment, null).key).toBeNull();
    expect(jsxDEV("p", { key: "fallback" }, undefined, false, undefined, null).key).toBe(
      "fallback",
    );
  });

  it("uses variadic children over props.children and does not invoke components", () => {
    let calls = 0;
    const component = () => {
      calls++;
      return "rendered";
    };
    const description = h(component, { children: "old" }, "new");
    expect(description.props.children).toBe("new");
    expect(calls).toBe(0);
    expect(h("div", { children: "kept" }).props.children).toBe("kept");
    expect(description.props.children).toBe("new");
  });

  it("flattens valid children without reading reactive values", () => {
    let reads = 0;
    const reactive = {
      $$zudoReactive: "zudo-react.reactive.v1",
      get value() {
        reads++;
        return "ok";
      },
    } as const;
    const description = h("b", null, "bold");
    expect(flattenChildren([null, true, "", [0, description, reactive]])).toEqual([
      "",
      0,
      description,
      reactive,
    ]);
    expect(reads).toBe(0);
  });

  it("rejects invalid children with their kind and path", () => {
    expect(() => flattenChildren(["ok", [() => "bad"]] as never)).toThrow(
      /function.*children\[1\]\[0\].*<root>/,
    );
    expect(() => flattenChildren(Promise.resolve("bad") as never)).toThrow(/promise/);
    expect(() => flattenChildren(Infinity as never)).toThrow(/number/);
  });
});
