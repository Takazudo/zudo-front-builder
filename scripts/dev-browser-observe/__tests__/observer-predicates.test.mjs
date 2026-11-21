import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import { HARNESS_DIR } from "../lib/util.mjs";

function loadEngine() {
  const sandbox = {};
  runInNewContext(
    readFileSync(join(HARNESS_DIR, "observer-init.js"), "utf8"),
    Object.assign(sandbox, { globalThis: sandbox }),
  );
  return sandbox.__dboPredicateEngine;
}

function el({ text = "", attrs = {}, style = {} } = {}) {
  return {
    textContent: text,
    getAttribute: (n) => (n in attrs ? attrs[n] : null),
    style,
  };
}

function env(map, docUUID = "doc-1") {
  return {
    docUUID,
    querySelectorAll: (s) => map[s] ?? [],
    getComputedStyle: (e) => ({ getPropertyValue: (p) => e.style[p] ?? "" }),
  };
}

describe("observer predicate engine (loaded from observer-init.js without a browser)", () => {
  const engine = loadEngine();

  it("exposes the engine and installs nothing else without window/document", () => {
    expect(typeof engine.evaluate).toBe("function");
    expect(engine.kinds).toEqual([
      "text",
      "attribute",
      "count",
      "absent",
      "computed-style",
      "document-uuid-not",
      "attribute-list",
      "all",
    ]);
  });

  it("text: includes/equals/excludes on the first match, trimmed by default", () => {
    const e = env({ "[t]": [el({ text: "  Article A tok-1  " })] });
    expect(engine.evaluate({ kind: "text", selector: "[t]", includes: "tok-1" }, e).matched).toBe(
      true,
    );
    expect(
      engine.evaluate({ kind: "text", selector: "[t]", equals: "Article A tok-1" }, e).matched,
    ).toBe(true);
    expect(engine.evaluate({ kind: "text", selector: "[t]", excludes: "tok-1" }, e).matched).toBe(
      false,
    );
    expect(
      engine.evaluate({ kind: "text", selector: "[t]", equals: "Article A tok-1", trim: false }, e)
        .matched,
    ).toBe(false);
    expect(
      engine.evaluate({ kind: "text", selector: "[missing]", includes: "x" }, e),
    ).toMatchObject({ matched: false, reason: "no-element" });
  });

  it("attribute: value ops and presence", () => {
    const e = env({ "[a]": [el({ attrs: { "data-x": "v-tok" } })] });
    expect(
      engine.evaluate({ kind: "attribute", selector: "[a]", name: "data-x", includes: "tok" }, e)
        .matched,
    ).toBe(true);
    expect(
      engine.evaluate({ kind: "attribute", selector: "[a]", name: "data-x", notEquals: "v-tok" }, e)
        .matched,
    ).toBe(false);
    expect(
      engine.evaluate({ kind: "attribute", selector: "[a]", name: "data-y", present: true }, e)
        .matched,
    ).toBe(false);
    expect(
      engine.evaluate({ kind: "attribute", selector: "[a]", name: "data-x", present: true }, e)
        .matched,
    ).toBe(true);
  });

  it("count and absence", () => {
    const e = env({ li: [el(), el()] });
    expect(engine.evaluate({ kind: "count", selector: "li", equals: 2 }, e).matched).toBe(true);
    expect(engine.evaluate({ kind: "count", selector: "li", equals: 3 }, e).matched).toBe(false);
    expect(engine.evaluate({ kind: "absent", selector: "li" }, e).matched).toBe(false);
    expect(engine.evaluate({ kind: "absent", selector: "nope" }, e).matched).toBe(true);
  });

  it("computed-style including a custom property", () => {
    const e = env({
      ":root": [el({ style: { "--observe-css-marker": " tok-9", color: "rgb(1, 2, 3)" } })],
    });
    expect(
      engine.evaluate(
        {
          kind: "computed-style",
          selector: ":root",
          property: "--observe-css-marker",
          equals: "tok-9",
        },
        e,
      ).matched,
    ).toBe(true);
    expect(
      engine.evaluate(
        { kind: "computed-style", selector: ":root", property: "color", includes: "1, 2" },
        e,
      ).matched,
    ).toBe(true);
    expect(
      engine.evaluate(
        { kind: "computed-style", selector: ":root", property: "--absent", equals: "x" },
        e,
      ).matched,
    ).toBe(false);
  });

  it("document-uuid-not detects a new document only", () => {
    expect(
      engine.evaluate({ kind: "document-uuid-not", value: "doc-1" }, env({}, "doc-1")).matched,
    ).toBe(false);
    expect(
      engine.evaluate({ kind: "document-uuid-not", value: "doc-1" }, env({}, "doc-2")).matched,
    ).toBe(true);
  });

  it("rejects malformed predicates instead of matching", () => {
    const e = env({ "[t]": [el({ text: "x" })] });
    expect(engine.evaluate({ kind: "text", selector: "[t]" }, e)).toMatchObject({ matched: false });
    expect(engine.evaluate({ kind: "bogus", selector: "[t]" }, e).error).toMatch(/unknown kind/);
    expect(engine.validate({ kind: "count", selector: "li", equals: "2" })).toContain(
      "count equals must be a number",
    );
    expect(engine.validate({ kind: "computed-style", selector: ":root", equals: "x" })).toContain(
      "property required",
    );
  });
});

describe("field-notes predicates (attribute-list, all)", () => {
  const engine = loadEngine();
  it("attribute-list matches the exact ordered list", () => {
    const links = ["/a/", "/b/"].map((h) => el({ attrs: { href: h } }));
    const e = env({ ".rel a": links });
    expect(
      engine.evaluate(
        { kind: "attribute-list", selector: ".rel a", name: "href", equals: ["/a/", "/b/"] },
        e,
      ),
    ).toMatchObject({ matched: true, actual: ["/a/", "/b/"] });
    expect(
      engine.evaluate(
        { kind: "attribute-list", selector: ".rel a", name: "href", equals: ["/b/", "/a/"] },
        e,
      ).matched,
    ).toBe(false);
    expect(
      engine.evaluate(
        { kind: "attribute-list", selector: ".rel a", name: "href", equals: ["/a/"] },
        e,
      ).matched,
    ).toBe(false);
    expect(
      engine.validate({ kind: "attribute-list", selector: "a", name: "href", equals: "x" }),
    ).toContain("attribute-list equals must be an array");
  });

  it("all requires every part and reports each actual", () => {
    const e = env({ h1: [el({ text: "T tok" })], ".eyebrow": [el({ text: "Testing" })] });
    const both = {
      kind: "all",
      of: [
        { kind: "text", selector: "h1", includes: "tok" },
        { kind: "text", selector: ".eyebrow", equals: "Testing" },
      ],
    };
    expect(engine.evaluate(both, e)).toMatchObject({ matched: true, actual: ["T tok", "Testing"] });
    expect(
      engine.evaluate({ ...both, of: [both.of[0], { ...both.of[1], equals: "Design" }] }, e)
        .matched,
    ).toBe(false);
    expect(engine.validate({ kind: "all", of: [] })).toEqual(["all needs a non-empty of[]"]);
    expect(
      engine.validate({ kind: "all", of: [{ kind: "count", selector: "x", equals: "1" }] })[0],
    ).toMatch(/of\[0\]: count equals must be a number/);
  });
});
