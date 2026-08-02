// Wasm-vs-native parity gate (zfb#1578, epic zfb#1572, wave 4).
//
// This is the epic's parity deliverable -- the reason @takazudo/zfb-md-wasm
// exists instead of pointing browser consumers at @mdx-js/mdx: the wasm
// build must produce output that is EXACTLY what zfb's native pipeline
// produces, not "close enough". This suite proves that on a fixed corpus.
//
// ## Oracle provenance (fancy-regex, not onig -- read before touching this)
//
// The committed oracle files under
// crates/zfb-md-wasm/tests/fixtures/parity/expected/*.json were produced by
// `cargo run -p zfb-md-wasm --bin generate_parity_oracle --features
// parity-oracle`
// (crates/zfb-md-wasm/tests/bin/generate_parity_oracle.rs), which links
// zfb-md-wasm's own **rlib** on the host triple -- literally the same
// src/lib.rs the wasm32-unknown-unknown cdylib under test here is compiled
// from, just a different target. zfb-md-wasm/Cargo.toml pulls zfb-content
// and zfb-render with `default-features = false, features =
// ["syntect-fancy"]`, and because `-p zfb-md-wasm` only builds THIS crate's
// own dependency subgraph (zfb-content is reached solely through
// zfb-render, never through the zfb/zfb-build binary crates whose
// dev-dependency cycle re-unifies the default syntect-onig backend for
// *their* graph), that feature selection is never widened back to onig
// inside this graph. Regenerate command (required-features gate --
// see Cargo.toml -- keeps this bin out of the npm package's own
// `pnpm build`, which cross-compiles this package for wasm32-unknown-unknown
// with no --lib/--bin filter):
//
//   cargo run -p zfb-md-wasm --bin generate_parity_oracle --features parity-oracle
//
// Verify independently that the oracle really is fancy-only:
//
//   cargo tree -p zfb-md-wasm -e normal | grep -i onig   # must print nothing
//
// This is what makes the exact-match gate below winnable: oracle and the
// wasm build under test share the identical highlighter backend. Reusing
// any onig-generated expected output here would be wrong -- see the epic
// issue's "oracle rule". onig-vs-fancy divergences are out of scope for
// this gate entirely (they live only in zfb#1573's informational test and
// the npm README's allowance list).
//
// ## What "byte-exact" means at this boundary
//
// The public npm API (`compile`/`renderHtml` in src/index.ts) parses the
// wasm module's JSON string return value into a plain object before
// returning it -- the raw wire string is not part of the public contract,
// so it can't be captured from outside src/** (which this suite must not
// touch). Both assertions below are therefore run per fixture:
//   1. `toStrictEqual` against the oracle file parsed the same way --
//      structural, type-exact equality (no fuzzy/tolerance matching).
//   2. `JSON.stringify(actual) === JSON.stringify(expected)` -- both sides
//      are plain JSON-shaped values (strings/numbers/booleans/null/arrays/
//      objects with a fixed key set). `JSON.parse` then `JSON.stringify`
//      normalizes away source formatting (the committed oracle files are
//      prettier-formatted multi-line JSON; the wasm wire string is compact)
//      while preserving key order, so comparing the re-serialized strings
//      is an exact value-identity check. The oracle and the wasm build run
//      the identical Rust serialization code (serde_json, same struct
//      field order), so nothing here has room to drift except a genuine
//      behavioral difference between the two targets.
//
// ## Corpus (FROZEN -- do not expand; see the epic issue's "bounded finish
// line" note). Single source of truth:
// crates/zfb-md-wasm/tests/fixtures/parity/manifest.json, shared with the
// Rust oracle generator. 13 fixtures, each drawn from an existing crate
// test's syntax and picked to cover one named tier from the epic issue:
// github alerts, code tabs, ruby, :::directives, CJK emphasis, GFM tables,
// heading links/toc, reading time, code enrichment, mermaid marker, mdx
// components + expressions, frontmatter variants, and a malformed-MDX
// diagnostic case.
//
// Test plan (declared per project testing discipline): Level 3 (build
// output) -- this compiles/renders through the REAL wasm binary executing
// in Node (loaded via the built `dist/` package, same as api.test.ts) and
// diffs its output against a real native build's output; nothing here is a
// mock or a hand-asserted substring.
//
// Blind spots (explicitly NOT covered here): browser execution of the wasm
// module (Node-only, per this package's existing test setup); onig-vs-fancy
// highlighting divergence (out of scope by the epic's oracle rule, see
// above); any corpus growth beyond the frozen 13 fixtures.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { describe, it, expect } from "vitest";

import { compile, renderHtml } from "../dist/index.js";
import type { ZfbMdWasmOptions } from "../dist/index.js";

const testDir = dirname(fileURLToPath(import.meta.url));
// crates/zfb-md-wasm/npm/test -> crates/zfb-md-wasm/tests/fixtures/parity.
// Lives under tests/ (sibling to npm/), not npm/test/fixtures/, because the
// Rust oracle generator (tests/bin/generate_parity_oracle.rs) needs to read
// the same manifest.json + input/ sources with a plain CARGO_MANIFEST_DIR
// join -- one fixture tree read by both toolchains, no duplication.
const fixturesDir = join(testDir, "..", "..", "tests", "fixtures", "parity");

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

// Sanity-check the frozen count so a future accidental edit to the manifest
// (add/remove a fixture) fails loudly here instead of just changing the
// number of `it` blocks silently.
const FROZEN_CORPUS_SIZE = 14;

describe(`wasm vs native fancy-regex oracle parity (${FROZEN_CORPUS_SIZE}-fixture frozen corpus)`, () => {
  it("manifest carries exactly the frozen corpus size", () => {
    expect(manifest.fixtures).toHaveLength(FROZEN_CORPUS_SIZE);
  });

  for (const fixture of manifest.fixtures) {
    it(`${fixture.slug} (${fixture.tier}) matches the native fancy-regex oracle byte-for-byte`, async () => {
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
