// @vitest-environment node
import { describe, expect, it } from "vite-plus/test";
import { escapeAttribute, escapeText } from "../../zudo-react/escape.js";

describe("HTML escaping", () => {
  it("escapes text and double-quoted attributes exactly once", () => {
    expect(escapeText(`&<>"`)).toBe(`&amp;&lt;&gt;"`);
    expect(escapeAttribute(`&<>"`)).toBe(`&amp;&lt;&gt;&quot;`);
  });
});
