import { describe, expect, it } from "vitest";
import { Fragment, Show, flattenChildren, h, isDescription } from "../../zudo-react/index.js";
import { jsx, jsxs } from "../../zudo-react/jsx-runtime.js";
import { jsxDEV } from "../../zudo-react/jsx-dev-runtime.js";
import { descriptionSite } from "../../zudo-react/description.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";

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

describe("zudo-react description sites", () => {
  const capture = Symbol.for("@takazudo/zfb/zudo-react/site-capture-v1");
  const flags = globalThis as Record<symbol, unknown>;

  it("keeps compiler source metadata from jsxDEV", () => {
    const source = { fileName: "pages/index.tsx", lineNumber: 4, columnNumber: 7 };
    const description = jsxDEV("p", null, undefined, false, source, null);
    expect(descriptionSite(description)).toEqual({
      kind: "source",
      file: "pages/index.tsx",
      line: 4,
      column: 7,
    });
    expect(Object.keys(description)).toEqual(["$$zudo", "type", "props", "key"]);
  });

  it("records no generated site unless the build host enables capture", () => {
    expect(descriptionSite(jsx("p", null))).toBeUndefined();
    flags[capture] = 1;
    try {
      const site = descriptionSite(jsx("p", null));
      expect(site).toEqual({
        kind: "generated",
        specifier: expect.stringMatching(/description\.test\.ts$/),
        line: expect.any(Number),
        column: expect.any(Number),
      });
      expect(descriptionSite(h("p", null))).toBeUndefined();
    } finally {
      delete flags[capture];
    }
  });

  it("attaches the failing element's site to the structured render error", () => {
    const source = { fileName: "components/field.tsx", lineNumber: 6, columnNumber: 7 };
    const failing = jsxDEV("input", { autoComplete: "off" }, undefined, false, source, null);
    let caught: unknown;
    try {
      renderToString(jsx("form", { children: failing }));
    } catch (error) {
      caught = error;
    }
    expect((caught as { diagnostic?: unknown }).diagnostic).toEqual({
      code: "ZR_PROP_DIALECT",
      path: "root",
      component: "static render",
      spelling: { name: "autoComplete", suggestion: "autocomplete" },
      site: { kind: "source", file: "components/field.tsx", line: 6, column: 7 },
    });
  });

  describe("never attributes a failure to the enclosing element", () => {
    const layout = { fileName: "layouts/base.tsx", lineNumber: 3, columnNumber: 5 };
    const inLayout = (child: unknown) =>
      jsxDEV("main", { children: child }, undefined, false, layout, null);
    function diagnosticOf(node: unknown): Record<string, unknown> {
      try {
        renderToString(node as never);
      } catch (error) {
        return (error as { diagnostic: Record<string, unknown> }).diagnostic;
      }
      throw new Error("expected a render failure");
    }
    const identity = { component: "Counter", build: "b1" };
    function Counter() {
      return jsx("p", { children: "1" });
    }
    function Outer() {
      return islandRoot(jsx(Counter, {}), { identity });
    }

    it.each([
      ["a promise child", () => inLayout(Promise.resolve("late")), "ZR_ASYNC_COMPONENT"],
      ["an invalid scalar child", () => inLayout(Symbol("bad")), "ZR_CHILD"],
      [
        "a nested island",
        () =>
          inLayout(islandRoot(jsx(Outer, {}), { identity: { component: "Outer", build: "b1" } })),
        "ZR_NESTED_ISLAND",
      ],
    ])("omits the site for %s", (_name, build, code) => {
      const diagnostic = diagnosticOf(build());
      expect(diagnostic.code).toBe(code);
      expect(diagnostic).not.toHaveProperty("site");
    });

    it("reports a failing Show description's own site", () => {
      const show = { fileName: "components/toggle.tsx", lineNumber: 8, columnNumber: 3 };
      const diagnostic = diagnosticOf(
        inLayout(jsxDEV(Show as never, { when: "yes" }, undefined, false, show, null)),
      );
      expect(diagnostic).toMatchObject({
        code: "ZR_CHILD",
        site: { kind: "source", file: "components/toggle.tsx", line: 8, column: 3 },
      });
    });
  });
});
