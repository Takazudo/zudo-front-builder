import { h } from "preact";
import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import RenderPlayground from "../render-playground";

describe("render playground security contract", () => {
  it("keeps the preview iframe sandbox token non-empty and scriptless", () => {
    const html = render(h(RenderPlayground, {}));

    expect(html).toContain('sandbox="allow-forms"');
    expect(html).not.toContain("allow-scripts");
    const previewIframe = html.match(/<iframe\b[^>]*title="Rendered HTML preview"[^>]*>/)?.[0];
    expect(previewIframe).toBeDefined();
    expect(previewIframe).toContain(
      "focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2",
    );
  });
});
