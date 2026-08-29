// Drift guard for nextest's two explicit e2e-heavy binary lists.
//
// `crates/zfb/tests/*.rs` is the ground truth: Cargo derives each integration
// test binary name verbatim from its file stem because crates/zfb/Cargo.toml
// has no explicit `[[test]]` targets. A source belongs to the binary list when
// it both spawns the zfb binary and passes an exact build/dev/preview
// subcommand, except for the reviewed exclusions below. `version_stamp` is
// classified separately because nextest registers its one heavy test by name.

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const ZFB_TESTS_DIR = join(REPO_ROOT, "crates", "zfb", "tests");
const NEXTEST_CONFIG = join(REPO_ROOT, ".config", "nextest.toml");

const HEAVY_GROUPS = ["e2e-heavy-locked", "e2e-heavy-unlocked"];

// Keep these reasons synchronized with .config/nextest.toml's documented
// exceptions. In particular, do not add `version_stamp`: its
// `version_stamp_from_env` test is heavy and covered by a test-name predicate.
const EXCLUSIONS = new Set([
  // "runs only `zfb check --skip-tsc`; no V8, no esbuild". A few parity
  // controls also spell `.arg("build")`, making this the one expected false
  // positive from the intentionally straightforward source heuristic.
  "check_command",
  // "calls `zfb_build::bundle` in-process — never spawns `zfb`, never boots
  // V8" (although it can shell out to esbuild).
  "framework_packages_no_pnpm",
  // Drives check/snapshot/build materialisation in-process with mocked
  // subprocess output; its header explicitly says no V8 or esbuild runs.
  "collections_outside_root_build_check_snapshot",
  // Pure framed-diagnostic rendering fixtures; never starts the zfb CLI.
  "diagnostics_fixtures",
  // Spawns only the cheap already-built `zfb --version` / `zfb -V` paths.
  "version_report",
]);

const PREDICATE_CLASSIFIED_STEMS = new Set(["version_stamp"]);

function integrationTestSources() {
  return readdirSync(ZFB_TESTS_DIR)
    .filter((entry) => /\.rs$/.test(entry))
    .map((entry) => ({
      stem: entry.slice(0, -".rs".length),
      source: readFileSync(join(ZFB_TESTS_DIR, entry), "utf8"),
    }));
}

function sourceHeuristicEvidence(source) {
  const spawnsZfb = /zfb_binary!\s*\(|CARGO_BIN_EXE(?:_zfb)?/.test(source);
  // Exact quoted array/argument values avoid treating names that merely
  // contain `_build_` as a build invocation.
  const argSubcommand = /\.arg\(\s*"(?:build|dev|preview)"\s*\)/s.test(source);
  const argsSubcommand = /\.args\(\s*\[\s*"(?:build|dev|preview)"(?:\s*,|\s*\])/s.test(source);
  return { spawnsZfb, argSubcommand, argsSubcommand };
}

function sourceHeuristicMatches(source) {
  const { spawnsZfb, argSubcommand, argsSubcommand } = sourceHeuristicEvidence(source);
  return spawnsZfb && (argSubcommand || argsSubcommand);
}

function heuristicCandidateStems(sources) {
  return new Set(
    sources.filter(({ source }) => sourceHeuristicMatches(source)).map(({ stem }) => stem),
  );
}

function parseHeavyGroupFilters(config) {
  const filters = new Map();
  for (const block of config.split("[[profile.default.overrides]]").slice(1)) {
    const group = block.match(/^test-group = '(e2e-heavy-(?:locked|unlocked))'$/m)?.[1];
    if (!group) continue;
    const filter = block.match(/^ filter = '([^'\n]+)'$/m)?.[1];
    if (!filter) throw new Error(`missing filter for nextest group ${group}`);
    if (filters.has(group)) throw new Error(`duplicate nextest group override: ${group}`);
    filters.set(group, filter);
  }
  return filters;
}

function binaryTerms(filter) {
  return new Set([...filter.matchAll(/binary\(=([^)]+)\)/g)].map((match) => match[1]));
}

function testTerms(filter) {
  return [...filter.matchAll(/test\(([^)]+)\)/g)].map((match) => match[1]);
}

function sorted(values) {
  return [...values].sort();
}

describe("nextest e2e-heavy drift guard", () => {
  const sources = integrationTestSources();
  const candidates = heuristicCandidateStems(sources);
  const derivedHeavy = new Set([...candidates].filter((stem) => !EXCLUSIONS.has(stem)));
  const filters = parseHeavyGroupFilters(readFileSync(NEXTEST_CONFIG, "utf8"));
  const locked = binaryTerms(filters.get("e2e-heavy-locked") ?? "");
  const unlocked = binaryTerms(filters.get("e2e-heavy-unlocked") ?? "");
  const configuredHeavy = new Set([...locked, ...unlocked]);

  it("reads both authoritative heavy-group filters", () => {
    expect(sorted(filters.keys())).toEqual(sorted(HEAVY_GROUPS));
  });

  it("classifies every integration-test stem", () => {
    const classified = new Set([...derivedHeavy, ...EXCLUSIONS, ...PREDICATE_CLASSIFIED_STEMS]);
    expect(sorted(classified)).toEqual(sorted(sources.map(({ stem }) => stem)));
  });

  it("derives exactly the configured heavy binary union", () => {
    expect(sorted(configuredHeavy)).toEqual(sorted(derivedHeavy));
  });

  it("has only the documented check_command source-heuristic false positive", () => {
    expect(sorted([...candidates].filter((stem) => EXCLUSIONS.has(stem)))).toEqual([
      "check_command",
    ]);
  });

  it("finds spawn and exact subcommand evidence in every configured binary source", () => {
    const sourcesByStem = new Map(sources.map(({ stem, source }) => [stem, source]));
    for (const stem of configuredHeavy) {
      const evidence = sourceHeuristicEvidence(sourcesByStem.get(stem) ?? "");
      expect(evidence.spawnsZfb, `${stem} must spawn zfb`).toBe(true);
      expect(
        evidence.argSubcommand || evidence.argsSubcommand,
        `${stem} must pass build/dev/preview as an exact subcommand`,
      ).toBe(true);
    }

    // This is the current `.args(["dev", ...])` spelling. Pin it separately
    // because workerd_parity_e2e also has `.arg(...)` calls, so final set
    // equality alone would not exercise the array branch of the heuristic.
    expect(
      sourceHeuristicEvidence(sourcesByStem.get("workerd_parity_e2e") ?? "").argsSubcommand,
    ).toBe(true);
  });

  it("does not register a binary in both serialization lanes", () => {
    expect(sorted([...locked].filter((stem) => unlocked.has(stem)))).toEqual([]);
  });

  it("keeps the embedded-V8 render-pipeline test predicate", () => {
    expect(testTerms(filters.get("e2e-heavy-locked") ?? "")).not.toContain(
      "=render_pipeline::tests::eval_deferred_paths_via_worker_embedded_v8_non_literal_paths",
    );
    expect(testTerms(filters.get("e2e-heavy-unlocked") ?? "")).toContain(
      "=render_pipeline::tests::eval_deferred_paths_via_worker_embedded_v8_non_literal_paths",
    );
  });

  it("keeps version_stamp heavy by exact test predicate", () => {
    expect(EXCLUSIONS.has("version_stamp")).toBe(false);
    expect(testTerms(filters.get("e2e-heavy-locked") ?? "")).not.toContain(
      "=version_stamp_from_env",
    );
    expect(testTerms(filters.get("e2e-heavy-unlocked") ?? "")).toContain("=version_stamp_from_env");
  });

  it("keeps the V8 host adapter module predicate", () => {
    expect(testTerms(filters.get("e2e-heavy-locked") ?? "")).not.toContain(
      "/^v8_host_adapter::tests::/",
    );
    expect(testTerms(filters.get("e2e-heavy-unlocked") ?? "")).toContain(
      "/^v8_host_adapter::tests::/",
    );
  });

  it("has no additional test-name predicate classes", () => {
    expect(testTerms(filters.get("e2e-heavy-locked") ?? "")).toEqual([]);
    expect(sorted(testTerms(filters.get("e2e-heavy-unlocked") ?? ""))).toEqual(
      sorted([
        "=render_pipeline::tests::eval_deferred_paths_via_worker_embedded_v8_non_literal_paths",
        "=version_stamp_from_env",
        "/^v8_host_adapter::tests::/",
      ]),
    );
  });
});
