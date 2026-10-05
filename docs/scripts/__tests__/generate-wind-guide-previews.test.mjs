import assert from "node:assert/strict";
import { test } from "node:test";
import { renderGuidePreviews } from "../generate-wind-guide-previews.mjs";

const example = { id: "sample", kind: "positive", scaffoldCss: ".wind-demo { color: red; }" };
const asset = {
  html: '<div class="block">Example</div>\n',
  css: ".block { display: block; }\n",
  head: '<base href="about:srcdoc">',
  config: { wind: { reset: "none", strict: true } },
};
const context = {
  examples: new Map([["guide-test", { examples: [example] }]]),
  assets: new Map([["guide-test/sample", asset]]),
};
const source =
  '## Existing heading\n\n{/* wind-preview: guide-test/sample "Visible result" */}\n{/* wind-preview:end */}\n';

test("guide blocks use exact verified source and native localized controls", () => {
  const rendered = renderGuidePreviews(source, "ja", context);
  assert.ok(rendered.startsWith("## Existing heading\n"));
  assert.ok(rendered.includes(`html={${JSON.stringify(asset.html)}}`));
  assert.ok(rendered.includes(`css={${JSON.stringify(asset.css)}}`));
  assert.ok(rendered.includes(`head={${JSON.stringify(asset.head)}}`));
  assert.match(rendered, /lang="ja" preflight=\{false\}/);
  assert.match(rendered, /この例の設定/);
  assert.equal(renderGuidePreviews(rendered, "ja", context), rendered);
});

test("diagnostic examples are textual and never runnable", () => {
  const negative = {
    ...example,
    kind: "expected-diagnostic",
    expectedDiagnostics: [{ code: "ZW005", severity: "error" }],
  };
  const ctx = { ...context, examples: new Map([["guide-test", { examples: [negative] }]]) };
  const rendered = renderGuidePreviews(source, "en", ctx);
  assert.match(rendered, /ZW005 \(error\)/);
  assert.ok(!rendered.includes("<HtmlPreview"));
  assert.ok(!rendered.includes("<WindPreviewEnhancer"));
});

test("missing records and malformed markers fail closed", () => {
  assert.throws(
    () => renderGuidePreviews(source.replace("sample", "missing"), "en", context),
    /Missing guide example/,
  );
  assert.throws(
    () => renderGuidePreviews(source.replace("wind-preview:end", "bad-end"), "en", context),
    /Malformed/,
  );
  assert.throws(
    () => renderGuidePreviews(source.replace('"Visible result"', "invalid"), "en", context),
    /Malformed/,
  );
});
