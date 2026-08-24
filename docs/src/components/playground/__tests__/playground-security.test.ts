import { h } from "preact";
import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import RenderPlayground from "../render-playground";

describe("render playground security contract", () => {
  it("keeps the preview iframe sandbox token non-empty and scriptless", () => {
    const html = render(h(RenderPlayground, {}));

    expect(html).toContain('sandbox="allow-forms"');
    expect(html).not.toContain("allow-scripts");
  });
});
