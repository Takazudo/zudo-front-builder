// Focused direct-highlight wasm-vs-native parity gate. This deliberately uses
// a separate corpus from `parity.test.ts`: compile/render's existing 14-case
// fixture set is frozen, while this file covers the new direct API only.
//
// Runs against BOTH the default (`.`) and highlight-only (`./highlight`,
// zfb#1849, epic zfb#1845) entries: the two wasm artifacts are compiled from
// the same `highlight_code_impl` Rust source (unconditional -- see
// crates/zfb-md-wasm/src/lib.rs), so they must highlight every fixture
// byte-for-byte identically.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { highlightCode as highlightCodeDefault, type HighlightCodeOptions } from "../dist/index.js";
import { highlightCode as highlightCodeOnly } from "../dist/highlight.js";

const testDir = dirname(fileURLToPath(import.meta.url));
const fixturesDir = join(testDir, "..", "..", "tests", "fixtures", "highlight-parity");

interface HighlightFixture {
  slug: string;
  code: string;
  options: HighlightCodeOptions;
}

interface HighlightManifest {
  fixtures: HighlightFixture[];
}

const manifest: HighlightManifest = JSON.parse(
  readFileSync(join(fixturesDir, "manifest.json"), "utf8"),
);
const FOCUSED_HIGHLIGHT_CORPUS_SIZE = 10;

const ENTRIES = [
  { label: "default (`.`) entry", highlightCode: highlightCodeDefault },
  { label: "highlight-only (`./highlight`) entry", highlightCode: highlightCodeOnly },
];

describe(`highlightCode wasm vs native oracle parity (${FOCUSED_HIGHLIGHT_CORPUS_SIZE}-fixture corpus)`, () => {
  it("manifest carries the intended focused corpus", () => {
    expect(manifest.fixtures).toHaveLength(FOCUSED_HIGHLIGHT_CORPUS_SIZE);
  });

  for (const entry of ENTRIES) {
    describe(entry.label, () => {
      for (const fixture of manifest.fixtures) {
        it(`${fixture.slug} matches the native fancy-regex oracle byte-for-byte`, async () => {
          const expected = JSON.parse(
            readFileSync(join(fixturesDir, "expected", `${fixture.slug}.json`), "utf8"),
          ) as unknown;
          const actual = await entry.highlightCode(fixture.code, fixture.options);

          expect(actual).toStrictEqual(expected);
          expect(JSON.stringify(actual)).toBe(JSON.stringify(expected));
        });
      }
    });
  }
});
