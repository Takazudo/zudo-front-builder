import { describe, expect, it } from "vite-plus/test";

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
  const mixed = [
    { label: "default", gzipSize: 100, gzipCeiling: 100 },
    { label: "highlight-only", gzipSize: 80, gzipCeiling: 100 },
    { label: "render-only", gzipSize: 150, gzipCeiling: 100 },
  ];

  it("reports equal, smaller, and larger sizes as warnings and never as errors", () => {
    for (const policy of [
      { allowOver: false, ci: false },
      { allowOver: true, ci: false },
      { allowOver: true, ci: true },
      ceilingPolicyFromArgv([], { ZFB_MD_WASM_ALLOW_OVER_CEILING: "1", CI: "true" }),
    ]) {
      const { errors, warnings } = evaluateCeilings(mixed, policy);
      expect(errors).toEqual([]);
      expect(warnings).toHaveLength(3);
      expect(warnings[0]).toContain("matches historical ceiling");
      expect(warnings[1]).toContain("delta -20");
      expect(warnings[2]).toContain("delta +50");
    }
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

  it("logs all four artifact summaries and a size warning without throwing", () => {
    const lines = [];
    const log = (msg) => lines.push(msg);
    const breaching = records.map((record, index) =>
      index === 2 ? { ...record, gzipSize: 150 } : record,
    );

    expect(() => reportCeilings(breaching, { allowOver: false, ci: true }, log)).not.toThrow();
    expect(lines.some((line) => line.includes("WARNING:") && line.includes("render-only"))).toBe(
      true,
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
