/**
 * Parity tests for `slugify` and `SlugAllocator` in `../slugify.ts`.
 *
 * The fixture consumed here is the SAME JSON file read by the Rust test in
 * `crates/zfb-content/tests/slugify_parity.rs`.  Any drift between the TS
 * port and the Rust implementation fails one side or the other.
 */

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { SlugAllocator, slugify } from "../slugify.js";

// Cross-package path: from packages/zfb/src/__tests__/ up to the repo root,
// then into the Rust crate that owns the shared fixture.
const here = dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = resolve(
  here,
  "../../../../crates/zfb-content/tests/fixtures/slugify-parity.json",
);

// ---------------------------------------------------------------------------
// Fixture schema
// ---------------------------------------------------------------------------

interface SlugifyCase {
  input: string;
  expected: string;
  _note?: string;
}

interface Heading {
  depth: number;
  text: string;
}

interface AllocateCase {
  strategy: "flat" | "hierarchical";
  headings: Heading[];
  expected: string[];
  _note?: string;
}

interface Fixture {
  slugify: SlugifyCase[];
  allocate: AllocateCase[];
}

const fixture: Fixture = JSON.parse(readFileSync(FIXTURE_PATH, "utf-8"));

// ---------------------------------------------------------------------------
// slugify parity
// ---------------------------------------------------------------------------

describe("slugify — parity with Rust heading_links::slugify", () => {
  for (const { input, expected, _note } of fixture.slugify) {
    it(`slugify(${JSON.stringify(input)}) → ${JSON.stringify(expected)}${_note ? `  [${_note}]` : ""}`, () => {
      expect(slugify(input)).toBe(expected);
    });
  }
});

// ---------------------------------------------------------------------------
// SlugAllocator parity
// ---------------------------------------------------------------------------

describe("SlugAllocator — parity with Rust SlugAllocator::allocate", () => {
  for (const { strategy, headings, expected, _note } of fixture.allocate) {
    const label = `strategy=${strategy}${_note ? `  [${_note}]` : ""}`;
    it(label, () => {
      const alloc = new SlugAllocator(strategy);
      const results = headings.map(({ depth, text }) => alloc.allocate(depth, slugify(text)));
      expect(results).toEqual(expected);
    });
  }
});
