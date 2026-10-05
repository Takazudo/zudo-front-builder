import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
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
