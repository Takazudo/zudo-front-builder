// Focused direct-highlight wasm-vs-native parity gate. This deliberately uses
// a separate corpus from `parity.test.ts`: compile/render's existing 14-case
// fixture set is frozen, while this file covers the new direct API only.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { highlightCode, type HighlightCodeOptions } from "../dist/index.js";

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
const FOCUSED_HIGHLIGHT_CORPUS_SIZE = 7;

describe(`highlightCode wasm vs native oracle parity (${FOCUSED_HIGHLIGHT_CORPUS_SIZE}-fixture corpus)`, () => {
  it("manifest carries the intended focused corpus", () => {
    expect(manifest.fixtures).toHaveLength(FOCUSED_HIGHLIGHT_CORPUS_SIZE);
  });

  for (const fixture of manifest.fixtures) {
    it(`${fixture.slug} matches the native fancy-regex oracle byte-for-byte`, async () => {
      const expected = JSON.parse(
        readFileSync(join(fixturesDir, "expected", `${fixture.slug}.json`), "utf8"),
      ) as unknown;
      const actual = await highlightCode(fixture.code, fixture.options);

      expect(actual).toStrictEqual(expected);
      expect(JSON.stringify(actual)).toBe(JSON.stringify(expected));
    });
  }
});
