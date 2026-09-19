import { describe, expect, it } from "vitest";

import {
  ceilingPolicyFromArgv,
  evaluateCeilings,
  legacyEnvVarWarning,
  reportCeilings,
} from "../../crates/zfb-md-wasm/npm/scripts/build.mjs";

// Pure functions only -- no wasm build runs in this file (zfb#3054, moved to
// a CLI-argument opt-in by zfb#3060/#3057).

describe("ceilingPolicyFromArgv", () => {
  it("allows over-ceiling only when --allow-over-ceiling is present in argv", () => {
    expect(ceilingPolicyFromArgv(["--allow-over-ceiling"], {}).allowOver).toBe(true);
    expect(ceilingPolicyFromArgv([], {}).allowOver).toBe(false);
    expect(ceilingPolicyFromArgv(["--other-flag"], {}).allowOver).toBe(false);
  });

  it("never reads the retired env var for the opt-in", () => {
    expect(ceilingPolicyFromArgv([], { ZFB_MD_WASM_ALLOW_OVER_CEILING: "1" }).allowOver).toBe(
      false,
    );
  });

  it("treats any non-empty CI string as CI (unchanged)", () => {
    expect(ceilingPolicyFromArgv([], { CI: "true" }).ci).toBe(true);
    expect(ceilingPolicyFromArgv([], { CI: "1" }).ci).toBe(true);
    expect(ceilingPolicyFromArgv([], { CI: "" }).ci).toBe(false);
    expect(ceilingPolicyFromArgv([], {}).ci).toBe(false);
  });
});

describe("legacyEnvVarWarning", () => {
  it("returns null when the retired env var is unset or empty", () => {
    expect(legacyEnvVarWarning({})).toBeNull();
    expect(legacyEnvVarWarning({ ZFB_MD_WASM_ALLOW_OVER_CEILING: "" })).toBeNull();
  });

  it("warns and names pnpm test:md-wasm:local for any non-empty value", () => {
    for (const value of ["1", "0", "true", "anything"]) {
      const warning = legacyEnvVarWarning({ ZFB_MD_WASM_ALLOW_OVER_CEILING: value });
      expect(warning).toContain("ZFB_MD_WASM_ALLOW_OVER_CEILING");
      expect(warning).toContain("no longer honored");
      expect(warning).toContain("pnpm test:md-wasm:local");
    }
  });
});

describe("evaluateCeilings", () => {
  const twoOver = [
    { label: "default", gzipSize: 100, gzipCeiling: 100 },
    { label: "highlight-only", gzipSize: 120, gzipCeiling: 100 },
    { label: "render-only", gzipSize: 150, gzipCeiling: 100 },
  ];

  it("reports no errors or warnings when every artifact is under its ceiling", () => {
    const allUnder = [
      { label: "default", gzipSize: 90, gzipCeiling: 100 },
      { label: "highlight-only", gzipSize: 100, gzipCeiling: 100 },
    ];
    expect(evaluateCeilings(allUnder, { allowOver: false, ci: false })).toEqual({
      errors: [],
      warnings: [],
    });
  });

  it("collects one error per over-ceiling artifact, never stopping at the first", () => {
    const { errors, warnings } = evaluateCeilings(twoOver, { allowOver: false, ci: false });
    expect(errors).toHaveLength(2);
    expect(errors[0]).toContain("highlight-only");
    expect(errors[1]).toContain("render-only");
    expect(warnings).toEqual([]);
  });

  it("downgrades breaches to warnings when the local opt-in is set outside CI", () => {
    const { errors, warnings } = evaluateCeilings(twoOver, { allowOver: true, ci: false });
    expect(errors).toEqual([]);
    expect(warnings).toHaveLength(2);
    expect(warnings[0]).toContain("highlight-only");
    expect(warnings[0]).toContain("over by 20");
    expect(warnings[0]).toContain("CI remains the oracle");
    expect(warnings[1]).toContain("render-only");
    expect(warnings[1]).toContain("over by 50");
  });

  it("refuses the opt-in outright when CI is set, naming the flag in a single error", () => {
    const { errors, warnings } = evaluateCeilings(twoOver, { allowOver: true, ci: true });
    expect(errors).toHaveLength(1);
    expect(errors[0]).toContain("--allow-over-ceiling");
    expect(warnings).toEqual([]);
  });

  // zfb#3057 regression: an exported ZFB_MD_WASM_ALLOW_OVER_CEILING must
  // never soften a breach again -- it is ignored, so policy.allowOver is
  // false and over-ceiling stats still error out.
  it("still errors on over-ceiling stats when only the retired env var is set", () => {
    const policy = ceilingPolicyFromArgv([], { ZFB_MD_WASM_ALLOW_OVER_CEILING: "1" });
    const { errors, warnings } = evaluateCeilings(twoOver, policy);
    expect(errors).toHaveLength(2);
    expect(warnings).toEqual([]);
  });
});

describe("reportCeilings", () => {
  const records = [
    {
      label: "default",
      entry: ".",
      gzipCeiling: 100,
      cdylibSize: 1,
      bindgenSize: 2,
      glueSize: 3,
      glueGzipSize: 4,
      finalSize: 5,
      gzipSize: 90,
    },
    {
      label: "highlight-only",
      entry: "./highlight",
      gzipCeiling: 100,
      cdylibSize: 1,
      bindgenSize: 2,
      glueSize: 3,
      glueGzipSize: 4,
      finalSize: 5,
      gzipSize: 90,
    },
    {
      label: "render-only",
      entry: "./render",
      gzipCeiling: 100,
      cdylibSize: 1,
      bindgenSize: 2,
      glueSize: 3,
      glueGzipSize: 4,
      finalSize: 5,
      gzipSize: 90,
    },
    {
      label: "parse-only",
      entry: "./parse",
      gzipCeiling: 100,
      cdylibSize: 1,
      bindgenSize: 2,
      glueSize: 3,
      glueGzipSize: 4,
      finalSize: 5,
      gzipSize: 90,
    },
  ];

  it("logs all four artifact summaries before throwing once, on breach", () => {
    const lines = [];
    const log = (msg) => lines.push(msg);
    const breaching = records.map((record, index) =>
      index === 2 ? { ...record, gzipSize: 150 } : record,
    );

    expect(() => reportCeilings(breaching, { allowOver: false, ci: false }, log)).toThrow(
      /render-only/,
    );

    for (const { label, entry } of records) {
      expect(lines.some((line) => line.includes(`${label} artifact (\`${entry}\` entry)`))).toBe(
        true,
      );
    }
  });

  it("does not throw when every artifact is under its ceiling", () => {
    const lines = [];
    expect(() =>
      reportCeilings(records, { allowOver: false, ci: false }, (msg) => lines.push(msg)),
    ).not.toThrow();
    expect(lines.length).toBeGreaterThan(0);
  });
});
