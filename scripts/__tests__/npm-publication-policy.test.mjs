import { describe, expect, it } from "vite-plus/test";
import {
  policyFromSourceFiles,
  policyForCommit,
  policyForSource,
  assertPackageRows,
  SUSPENDED_PACKAGE,
  SOURCE_METADATA_SCHEMA,
} from "../release/npm-publication-policy.mjs";
const text = JSON.stringify({
  schema: "zfb-npm-publication-policy/v1",
  suspended: [SUSPENDED_PACKAGE],
});
describe("immutable source npm publication policy", () => {
  it("excludes only the suspended carrier, retaining publication order", () => {
    const legacy = policyFromSourceFiles();
    const current = policyFromSourceFiles({ policyText: text });
    expect(legacy.names).toHaveLength(11);
    expect(current.names).toEqual(legacy.names.filter((n) => n !== SUSPENDED_PACKAGE));
  });
  it("keeps v6.1.4 legacy contract even under new control code", () => {
    expect(policyForCommit("fe7e24d61b04795b41fba50f2c353f61ca8f9081").names).toContain(
      SUSPENDED_PACKAGE,
    );
  });
  it("preserves only the explicit historical recovery exception", () => {
    expect(() => policyFromSourceFiles({ hasSlugify: false })).toThrow(/slugify/);
    expect(
      policyFromSourceFiles({ hasSlugify: false, mode: "recovery-no-provenance" }).names,
    ).toHaveLength(10);
    expect(() =>
      policyFromSourceFiles({
        policyText: text,
        hasSlugify: false,
        mode: "recovery-no-provenance",
      }),
    ).toThrow(/slugify/);
  });
  it("resumes the carrier when the source policy suspends nothing", () => {
    const resumed = policyFromSourceFiles({
      policyText: JSON.stringify({ schema: "zfb-npm-publication-policy/v1", suspended: [] }),
    });
    expect(resumed.suspended).toEqual([]);
    expect(resumed.names).toHaveLength(11);
    expect(resumed.names).toContain(SUSPENDED_PACKAGE);
  });
  it.each([
    { schema: "bad", suspended: [SUSPENDED_PACKAGE] },
    { schema: "zfb-npm-publication-policy/v1", suspended: [SUSPENDED_PACKAGE, SUSPENDED_PACKAGE] },
    { schema: "zfb-npm-publication-policy/v1", suspended: ["unknown"] },
  ])("rejects malformed or unexpected source policy %j", (data) => {
    expect(() => policyFromSourceFiles({ policyText: JSON.stringify(data) })).toThrow();
  });
  it("requires gitHead only for sources declaring the new policy schema", () => {
    expect(policyFromSourceFiles().requiresGitHead).toBe(false);
    expect(policyFromSourceFiles({ policyText: text }).requiresGitHead).toBe(false);
    const policyText = JSON.stringify({ schema: SOURCE_METADATA_SCHEMA, suspended: [] });
    expect(policyFromSourceFiles({ policyText }).requiresGitHead).toBe(true);
    expect(() =>
      policyFromSourceFiles({
        policyText: JSON.stringify({
          schema: SOURCE_METADATA_SCHEMA,
          suspended: [],
          requiresGitHead: false,
        }),
      }),
    ).toThrow(/invalid source npm publication policy/);
  });
  it("rejects missing, duplicate, unknown, reordered, wrong-version rows", () => {
    const policy = policyFromSourceFiles({ policyText: text });
    const rows = policy.packages.map((p) => ({ ...p, version: "7.0.0" }));
    expect(() => assertPackageRows(rows, policy, "7.0.0")).not.toThrow();
    for (const invalid of [
      rows.slice(1),
      [...rows, rows[0]],
      [rows[1], rows[0], ...rows.slice(2)],
      rows.map((p, i) => (i ? p : { ...p, name: "unknown" })),
      rows.map((p, i) => (i ? p : { ...p, version: "6.0.1" })),
    ]) {
      expect(() => assertPackageRows(invalid, policy, "7.0.0")).toThrow();
    }
  });
  it("fails closed for incomplete remote trees and API failures", () => {
    expect(() =>
      policyForSource("a".repeat(40), { gh: () => JSON.stringify({ truncated: true, tree: [] }) }),
    ).toThrow(/incomplete/);
    expect(() =>
      policyForSource("a".repeat(40), {
        gh: () => {
          throw new Error("forbidden");
        },
      }),
    ).toThrow(/forbidden/);
  });
});
