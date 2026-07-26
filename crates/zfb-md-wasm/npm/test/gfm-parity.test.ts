// Wasm-vs-native parity gate for GFM task lists and footnotes (zfb#2028,
// epic zfb#2021, wave 5).
//
// This is a SEPARATE suite/corpus from parity.test.ts on purpose. That
// suite's own header comment freezes its 14-fixture corpus per the zfb#1578
// epic's bounded-finish-line decision -- task lists and footnotes are a
// later, unrelated GFM Waves 2-4 addition (epic zfb#2021), so they get their
// own tree instead of reopening that frozen one. Same mechanism otherwise:
// this compiles/renders through the REAL wasm binary (loaded via the built
// `dist/` package) and diffs its output against a real native build's
// output, byte-exact.
//
// ## Oracle provenance
//
// The committed oracle files under
// crates/zfb-md-wasm/tests/fixtures/gfm-parity/expected/*.json were produced
// by the SAME generator parity.test.ts uses
// (crates/zfb-md-wasm/tests/bin/generate_parity_oracle.rs), pointed at this
// tree via its optional fixtures-subdir CLI arg:
//
//   cargo run -p zfb-md-wasm --bin generate_parity_oracle --features parity-oracle -- gfm-parity
//
// See parity.test.ts's own header comment for why this shares the same
// fancy-regex-backed rlib build as the wasm32-unknown-unknown cdylib under
// test, and why parse-then-compare (not raw string diff) is the right
// boundary for the byte-exact assertion below.
//
// Test plan (declared per project testing discipline): Level 3 (build
// output) -- real wasm binary executing in Node vs. a real native build,
// nothing mocked.
//
// What each fixture proves:
// - `task-list`: Option B's minimal checkbox rendering (issue #2024) is
//   identical between wasm and native for checked/unchecked/nested items
//   and a plain (non-task-list) sibling item.
// - `footnotes`: the document-level footnote model (issue #2025) is
//   identical between wasm and native for first-reference-order numbering,
//   a repeated reference's distinct backreference id, an empty-slugifying
//   label's numeric-id fallback, and duplicate-definition collapse.
// - `footnotes-jsx`: the JSX-emit path (issue #2027) is identical between
//   wasm and native for a footnote reference+definition nested inside an
//   MDX JSX element body.
//
// Blind spots (explicitly NOT covered here): browser execution of the wasm
// module (Node-only, matching parity.test.ts's own stated blind spot);
// onig-vs-fancy highlighting divergence (out of scope, same as
// parity.test.ts -- these fixtures carry no code fences).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { describe, it, expect } from "vitest";

import { compile, renderHtml } from "../dist/index.js";
import type { ZfbMdWasmOptions } from "../dist/index.js";

const testDir = dirname(fileURLToPath(import.meta.url));
// crates/zfb-md-wasm/npm/test -> crates/zfb-md-wasm/tests/fixtures/gfm-parity.
// Lives under tests/ (sibling to npm/), matching parity.test.ts's own layout
// rationale: the Rust oracle generator needs a plain CARGO_MANIFEST_DIR join.
const fixturesDir = join(testDir, "..", "..", "tests", "fixtures", "gfm-parity");

interface ManifestFixture {
  slug: string;
  tier: "compile" | "renderHtml";
  input: string;
  options: Record<string, unknown>;
}

interface Manifest {
  fixtures: ManifestFixture[];
}

const manifest: Manifest = JSON.parse(readFileSync(join(fixturesDir, "manifest.json"), "utf8"));

describe("wasm vs native fancy-regex oracle parity (GFM task lists and footnotes)", () => {
  for (const fixture of manifest.fixtures) {
    it(`${fixture.slug} (${fixture.tier}) matches the native oracle byte-for-byte`, async () => {
      const source = readFileSync(join(fixturesDir, fixture.input), "utf8");
      const expectedRaw = readFileSync(
        join(fixturesDir, "expected", `${fixture.slug}.json`),
        "utf8",
      );
      const expected = JSON.parse(expectedRaw) as unknown;

      const options = fixture.options as ZfbMdWasmOptions;
      const actual =
        fixture.tier === "compile"
          ? await compile(source, options)
          : await renderHtml(source, options);

      expect(actual).toStrictEqual(expected);
      expect(JSON.stringify(actual)).toBe(JSON.stringify(expected));
    });
  }
});
