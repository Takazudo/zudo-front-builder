import { h } from "preact";
import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import {
  NOTE_DIRECTIVE_FEATURES,
  NOTE_DIRECTIVE_SAMPLE,
  noteDirectiveFeatures,
} from "../admonition-sample";
import CompilePlayground from "../compile-playground";
import ParsePlayground from "../parse-playground";
import RenderPlayground from "../render-playground";

describe("playground admonition samples", () => {
  it("pins the registered note directive sample and feature options", () => {
    expect(NOTE_DIRECTIVE_SAMPLE).toMatchObject({
      id: "admonition-directive",
      label: "Admonition directive",
    });
    expect(NOTE_DIRECTIVE_SAMPLE.value).toContain(":::note[Heads up]");
    expect(noteDirectiveFeatures(true)).toBe(NOTE_DIRECTIVE_FEATURES);
    expect(noteDirectiveFeatures(false)).toBeUndefined();
  });

  it.each([
    ["renderHtml", RenderPlayground],
    ["compile", CompilePlayground],
  ] as const)("offers the sample and explicit feature toggle in %s", (_name, Component) => {
    const html = render(h(Component, {}));

    expect(html).toContain("Admonition directive");
    expect(html).toContain("pipeline.features.directives.note = &quot;Note&quot;");
  });

  it("offers the sample beside parseToAst's generic directives option", () => {
    const html = render(h(ParsePlayground, {}));

    expect(html).toContain("Admonition directive");
    expect(html).toMatch(/<span class="font-mono">directives<\/span>/);
  });
});
