import { describe, expect, it } from "vitest";

import {
  ceilingPolicyFromEnv,
  evaluateCeilings,
  reportCeilings,
} from "../../crates/zfb-md-wasm/npm/scripts/build.mjs";

// Pure functions only -- no wasm build runs in this file (zfb#3054).

describe("ceilingPolicyFromEnv", () => {
  it("allows over-ceiling only for the exact string '1'", () => {
    expect(ceilingPolicyFromEnv({ ZFB_MD_WASM_ALLOW_OVER_CEILING: "1" }).allowOver).toBe(true);
    expect(ceilingPolicyFromEnv({ ZFB_MD_WASM_ALLOW_OVER_CEILING: "true" }).allowOver).toBe(false);
    expect(ceilingPolicyFromEnv({ ZFB_MD_WASM_ALLOW_OVER_CEILING: "0" }).allowOver).toBe(false);
    expect(ceilingPolicyFromEnv({ ZFB_MD_WASM_ALLOW_OVER_CEILING: "" }).allowOver).toBe(false);
    expect(ceilingPolicyFromEnv({}).allowOver).toBe(false);
  });

  it("treats any non-empty CI string as CI", () => {
    expect(ceilingPolicyFromEnv({ CI: "true" }).ci).toBe(true);
    expect(ceilingPolicyFromEnv({ CI: "1" }).ci).toBe(true);
    expect(ceilingPolicyFromEnv({ CI: "" }).ci).toBe(false);
    expect(ceilingPolicyFromEnv({}).ci).toBe(false);
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

  it("refuses the opt-in outright when CI is set, naming the variable in a single error", () => {
    const { errors, warnings } = evaluateCeilings(twoOver, { allowOver: true, ci: true });
    expect(errors).toHaveLength(1);
    expect(errors[0]).toContain("ZFB_MD_WASM_ALLOW_OVER_CEILING");
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
