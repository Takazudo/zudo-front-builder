import { h } from "preact";
import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import OptionRow from "../option-row";

describe("playground option row layout", () => {
  it("keeps checkbox rows compact without shrinking the label touch target", () => {
    const html = render(
      h(OptionRow, {
        label: "pipeline.gfm.table",
        checked: true,
        onChange: () => undefined,
      }),
    );

    const rowClasses = html.match(/^<div class="([^"]+)">/)?.[1];
    const labelClasses = html.match(/<label class="([^"]+)">/)?.[1];

    expect(rowClasses).toBeDefined();
    expect(rowClasses).not.toContain("py-vsp-");
    expect(labelClasses).toContain("min-h-[44px]");
    expect(labelClasses).toContain("min-w-0");
    expect(labelClasses).toContain("[overflow-wrap:anywhere]");
    expect(labelClasses).not.toContain("py-vsp-");
  });

  it("keeps bottom spacing when a row exposes nested options", () => {
    const html = render(
      h(
        OptionRow,
        {
          label: "pipeline.codeHighlight",
          checked: true,
          onChange: () => undefined,
        },
        h("span", null, "Nested option"),
      ),
    );

    const rowClasses = html.match(/^<div class="([^"]+)">/)?.[1];

    expect(rowClasses).toContain("pb-vsp-2xs");
  });
});
