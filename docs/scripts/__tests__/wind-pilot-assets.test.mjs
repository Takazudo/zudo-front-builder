import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { CODE_BLOCK_ENHANCER_SCRIPT } from "@takazudo/zudo-doc/code-syntax";
import { buildWindPreviewEnhancerScript } from "../../src/components/wind-preview-enhancer-script.mjs";
import { validateEditorial } from "../wind-reference-editorial.mjs";
import { exampleSource, REPO_ROOT } from "../wind-preview-assets.mjs";

const sha256 = (content) => createHash("sha256").update(content).digest("hex");

test("Gap and Padding public HTML/CSS assets match their shared records and manifest", () => {
  const manifest = JSON.parse(
    readFileSync(join(REPO_ROOT, "docs/public/wind-examples/manifest.json"), "utf8"),
  );

  for (const family of ["gap", "padding"]) {
    const record = JSON.parse(
      readFileSync(join(REPO_ROOT, `docs/wind-examples/${family}.json`), "utf8"),
    );
    for (const example of record.examples) {
      const asset = manifest.examples.find(
        (entry) => entry.family === family && entry.id === example.id,
      );
      assert.ok(asset, `manifest is missing ${family}/${example.id}`);
      assert.equal(asset.kind, "positive");

      const html = readFileSync(join(REPO_ROOT, "docs/public", asset.htmlPath), "utf8");
      const css = readFileSync(join(REPO_ROOT, "docs/public", asset.cssPath), "utf8");
      assert.equal(html, exampleSource(example), `${family}/${example.id} HTML source bytes`);
      assert.equal(sha256(html), asset.htmlSha256, `${family}/${example.id} HTML digest`);
      assert.equal(sha256(css), asset.cssSha256, `${family}/${example.id} CSS digest`);
    }
  }
});

test("localized generated pilot pages install the enhancer once before their previews", () => {
  for (const locale of ["en", "ja"]) {
    const prefix = locale === "en" ? "docs/src/content/docs" : "docs/src/content/docs-ja";
    for (const family of ["gap", "padding"]) {
      const page = readFileSync(
        join(REPO_ROOT, `${prefix}/zudo-wind/utilities/${family}.mdx`),
        "utf8",
      );
      const marker = "<WindPreviewEnhancer />";
      assert.equal(page.split(marker).length - 1, 1, `${locale}/${family} enhancer count`);
      assert.ok(
        page.indexOf(marker) < page.indexOf("<HtmlPreview "),
        `${locale}/${family} installs the enhancer before previews mount`,
      );
    }
  }
});

test("late preview code uses the pinned enhancer behind a unique, body-swap-safe observer", () => {
  const script = buildWindPreviewEnhancerScript(CODE_BLOCK_ENHANCER_SCRIPT);
  assert.equal(script.split("function enhanceCodeBlocks()").length - 1, 1);
  assert.equal(script.split("code-btn-copy").length - 1, 1);
  assert.match(script, /window\.__zfbWindPreviewCodeEnhancerInstalled\) return;/);
  assert.ok(
    script.indexOf("window.__zfbWindPreviewCodeEnhancerInstalled = true;") <
      script.indexOf("function readWrapMode()"),
    "the once-only guard runs before the native enhancer installs listeners",
  );
  assert.match(script, /\.zd-html-preview-code pre\.hi-root/);
  assert.match(
    script,
    /latePreviewCodeObserver\.observe\(document\.documentElement, \{ childList: true, subtree: true \}\)/,
  );
  assert.equal(script.split("// Run on initial load.").length - 1, 1);
  assert.throws(
    () =>
      buildWindPreviewEnhancerScript(
        CODE_BLOCK_ENHANCER_SCRIPT.replace("Run on initial load", "initial scan"),
      ),
    /initial-scan marker must occur exactly once/,
  );
});

test("editorial validation honors only catalog-marked optional font leading", () => {
  const candidate = "text-[1rem]";
  const catalog = JSON.parse(
    readFileSync(join(REPO_ROOT, "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json"), "utf8"),
  );
  const entries = new Map(catalog.entries.map((entry) => [entry.id, entry]));
  const fontSize = entries.get("v1.text.size");
  const textColor = entries.get("v1.text.color");
  const sample = { id: "font-size", kind: "positive", utilities: [candidate] };
  const recordFor = (entry) => ({
    schemaVersion: 1,
    family: "typography",
    lookup: [{ entry: entry.id, candidate, example: sample.id }],
    locales: Object.fromEntries(
      ["en", "ja"].map((locale) => [
        locale,
        {
          purpose: "Purpose",
          setup: "Setup",
          customValues: "Custom values",
          examples: [{ id: sample.id, title: "Text utility", description: "A text utility." }],
        },
      ]),
    ),
  });
  const examples = { examples: [sample] };
  const assets = new Map([
    ["typography/font-size", { css: ".text-\\[1rem\\] { font-size: 1rem; }" }],
    ["typography/color", { css: ".text-\\[1rem\\] { font-size: 1rem; }" }],
  ]);
  const validate = (entry) =>
    validateEditorial(
      recordFor(entry),
      { id: "typography", entries: [entry.id] },
      examples,
      entries,
      assets,
    );

  assert.doesNotThrow(() => validate(fontSize));
  assert.throws(() => validate(textColor), /catalog properties color/);
});
