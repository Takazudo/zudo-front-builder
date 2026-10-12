import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

import {
  REASONS,
  describeDeltas,
  diffCeilings,
  measuredTotals,
  parseArgs,
  readmeSection,
  rebaselineContract,
  rewriteTestPins,
} from "../rebaseline-island-size-linux.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const contract = JSON.parse(
  readFileSync(join(root, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
);
const testSource = readFileSync(
  join(root, "scripts/__tests__/island-size-budget.test.mjs"),
  "utf8",
);

// Totals in the shape measuredTotals() derives from normalize-identity.mjs
// results: raw and gzipNormalized are the ceilings, gzip rides along as
// evidence. The synthetic actual gzip is the ceiling plus a fixed offset.
function totalsFrom(base, edits = {}) {
  const totals = {};
  for (const mode of ["workspace", "packed"]) {
    totals[mode] = {};
    for (const [fixture, ceiling] of Object.entries(base.ceilings[mode])) {
      totals[mode][fixture] = {
        raw: ceiling.raw,
        gzipNormalized: ceiling.gzipNormalized,
        gzip: ceiling.gzipNormalized === 0 ? 0 : ceiling.gzipNormalized + 15,
      };
    }
  }
  for (const [path, value] of Object.entries(edits)) {
    const [mode, fixture] = path.split("/");
    totals[mode][fixture] = { ...totals[mode][fixture], ...value };
  }
  return totals;
}

// A schemaVersion-1 actual-gzip contract, the shape metric-migration consumes.
function legacyContract(base) {
  const legacy = {};
  for (const [key, value] of Object.entries(base)) {
    if (key === "metric" || key === "normalizationVersion" || key === "normalizerSha256") continue;
    legacy[key] = structuredClone(value);
  }
  legacy.schemaVersion = 1;
  legacy.outcome = "no-product-code-change";
  legacy.allowance = { raw: 0, gzip: 0 };
  for (const mode of ["workspace", "packed"]) {
    for (const [fixture, ceiling] of Object.entries(base.ceilings[mode])) {
      legacy.ceilings[mode][fixture] = {
        raw: ceiling.raw,
        gzip: ceiling.gzipNormalized === 0 ? 0 : ceiling.gzipNormalized + 15,
      };
    }
  }
  return legacy;
}

const evidence = {
  version: "9.9.9",
  pnpmLockSha256: "a".repeat(64),
  normalizerSha256: "e".repeat(64),
  sourceSha: "b".repeat(40),
  headSha: "c".repeat(40),
  runId: 1,
  jobId: 2,
  runLabel: "Release probe run for v9.9.9",
};

describe("rebaseline-island-size-linux", () => {
  it("re-pins a release bump whose raw and gzipNormalized totals did not move", () => {
    const totals = totalsFrom(contract, {
      "workspace/model": { gzip: contract.ceilings.workspace.model.gzipNormalized + 18 },
      "packed/json-api": { gzip: contract.ceilings.packed["json-api"].gzipNormalized + 13 },
    });
    const { contract: next, gzipEvidence } = rebaselineContract(
      contract,
      totals,
      evidence,
      "release-bump",
    );
    expect(next.schemaVersion).toBe(2);
    expect(next.metric).toBe("gzip-identity-normalized");
    expect(next.normalizationVersion).toBe(1);
    expect(next.normalizerSha256).toBe(evidence.normalizerSha256);
    expect(next.ceilings).toEqual(contract.ceilings);
    expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(next.outcome).toBe("release-version-stamp");
    expect(next.baselineSourceSha).toBe(evidence.headSha);
    expect(next.toolchain.packageVersion).toBe("9.9.9");
    expect(next.toolchain.pnpmLockSha256).toBe(evidence.pnpmLockSha256);
    expect(next.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 1,
      jobId: 2,
      sourceSha: evidence.sourceSha,
    });
    expect(next.baselineEvidence.note).toContain(
      "PR merge source " + evidence.sourceSha + " (probe head " + evidence.headSha + ")",
    );
    expect(next.baselineEvidence.note).toContain(
      `Raw and gzipNormalized totals did not change from the v${contract.toolchain.packageVersion} contract`,
    );
    expect(next.platformStatus).toContain("Every raw and gzipNormalized total is unchanged");
    expect(next.platformStatus).not.toContain("no-product-code-change");
    expect(next.outcome).not.toBe("no-product-code-change");
    // The previous contract pins gzipNormalized, so the synthetic +15 actual
    // gzip has no prior value to compare with and is not reported as movement.
    expect(gzipEvidence).toEqual([]);
    expect(contract.toolchain.packageVersion).not.toBe("9.9.9");
  });

  it("refuses a raw change under every reason", () => {
    const totals = totalsFrom(contract, {
      "packed/show-for": { raw: contract.ceilings.packed["show-for"].raw + 1 },
    });
    expect(diffCeilings(contract, totals).rawChanges).toHaveLength(1);
    for (const reason of ["release-bump", "runtime-identity"]) {
      expect(() => rebaselineContract(contract, totals, evidence, reason)).toThrow(
        /raw totals changed \(packed\/show-for\)/,
      );
    }
    const legacy = legacyContract(contract);
    expect(() => rebaselineContract(legacy, totals, evidence, "metric-migration")).toThrow(
      /raw totals changed/,
    );
  });

  it("refuses a gzipNormalized change: the token is normalized away, so code moved", () => {
    const totals = totalsFrom(contract, {
      "workspace/model": { gzipNormalized: contract.ceilings.workspace.model.gzipNormalized + 3 },
    });
    expect(diffCeilings(contract, totals).normalizedChanges).toEqual([
      { mode: "workspace", fixture: "model", delta: 3 },
    ]);
    for (const reason of ["release-bump", "runtime-identity"]) {
      expect(() => rebaselineContract(contract, totals, evidence, reason)).toThrow(
        /gzipNormalized totals changed \(workspace\/model \+3\); the identity token is normalized away, so this is product code moving, not a /,
      );
    }
  });

  it("migrates a schemaVersion-1 actual-gzip contract from replayed bytes", () => {
    const legacy = legacyContract(contract);
    const totals = totalsFrom(contract);
    expect(diffCeilings(legacy, totals)).toMatchObject({
      rawChanges: [],
      normalizedChanges: [],
      gzipChanges: [],
    });
    const { contract: next, previousVersion } = rebaselineContract(
      legacy,
      totals,
      { ...evidence, version: "4.3.1" },
      "metric-migration",
    );
    expect(previousVersion).toBe(contract.toolchain.packageVersion);
    expect(Object.keys(next).slice(0, 5)).toEqual([
      "schemaVersion",
      "metric",
      "normalizationVersion",
      "normalizerSha256",
      "decisionIssue",
    ]);
    expect(next.schemaVersion).toBe(2);
    expect(next.metric).toBe("gzip-identity-normalized");
    expect(next.normalizerSha256).toBe(evidence.normalizerSha256);
    expect(next.outcome).toBe("metric-migration");
    expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(next.ceilings).toEqual(contract.ceilings);
    expect(next.baselineEvidence.event).toBe("ci-artifact-replay");
    expect(next.baselineEvidence.note).toContain("not converted arithmetically");
    expect(next.baselineEvidence.note).toContain(
      "re-verifies these values on the integrated tree in CI",
    );
    expect(next.platformStatus).toContain("replayed by normalize-identity.mjs");
    expect(next.platformStatus).toContain("not comparable with any earlier gzip ceiling");
    expect(() => rebaselineContract(contract, totals, evidence, "metric-migration")).toThrow(
      /--reason metric-migration needs a schemaVersion 1 contract, found schemaVersion 2/,
    );
    expect(() => rebaselineContract(legacy, totals, evidence, "release-bump")).toThrow(
      /--reason release-bump needs a schemaVersion 2 contract, found schemaVersion 1/,
    );
  });

  it("re-pins a runtime-identity change with its own outcome and prose", () => {
    const { contract: next } = rebaselineContract(
      contract,
      totalsFrom(contract),
      { ...evidence, sourceSha: evidence.headSha, runLabel: "Runtime-identity re-pin" },
      "runtime-identity",
    );
    expect(next.outcome).toBe("runtime-identity-repin");
    expect(next.platformStatus).toContain("at source " + evidence.headSha + ".");
    expect(next.platformStatus).toContain(
      "equal totals alone do not prove byte-identical product code",
    );
    expect(Object.keys(REASONS)).toEqual(["release-bump", "metric-migration", "runtime-identity"]);
  });

  it("refuses a missing fixture", () => {
    const totals = totalsFrom(contract);
    delete totals.workspace["blog-theme"];
    expect(() => rebaselineContract(contract, totals, evidence, "release-bump")).toThrow(
      /fixture set differs from the contract \(workspace\/blog-theme\)/,
    );
  });

  it("derives totals from normalizer results and describes evidence deltas", () => {
    const results = {
      workspace: { "no-island": { shipped: { raw: 0, gzip: 0, gzipNormalized: 0 } } },
      packed: { "no-island": { shipped: { raw: 10, gzip: 9, gzipNormalized: 8 } } },
    };
    expect(measuredTotals(results)).toEqual({
      workspace: { "no-island": { raw: 0, gzipNormalized: 0, gzip: 0 } },
      packed: { "no-island": { raw: 10, gzipNormalized: 8, gzip: 9 } },
    });
    expect(
      describeDeltas([
        { mode: "workspace", fixture: "model", delta: 3 },
        { mode: "workspace", fixture: "show-for", delta: 3 },
        { mode: "packed", fixture: "json-api", delta: -2 },
      ]),
    ).toBe("workspace model and show-for +3; packed json-api -2");
  });

  it("validates CLI arguments before touching any file", () => {
    const ok = [
      "--artifact",
      "dir",
      "--run-id",
      "12",
      "--job-id",
      "34",
      "--head-sha",
      "d".repeat(40),
      "--reason",
      "release-bump",
    ];
    expect(parseArgs(ok)).toMatchObject({
      "run-id": "12",
      "head-sha": "d".repeat(40),
      reason: "release-bump",
    });
    expect(() => parseArgs(ok.slice(0, 8))).toThrow(/required --reason/);
    expect(() => parseArgs(ok.slice(0, 6).concat(ok.slice(8)))).toThrow(/required --head-sha/);
    expect(() => parseArgs(ok.map((v) => (v === "12" ? "12a" : v)))).toThrow(/--run-id must be/);
    expect(() => parseArgs(ok.map((v) => (v === "d".repeat(40) ? "abc123" : v)))).toThrow(
      /--head-sha must be/,
    );
    expect(() => parseArgs(ok.map((v) => (v === "release-bump" ? "gzip-shift" : v)))).toThrow(
      /--reason must be one of release-bump, metric-migration, runtime-identity/,
    );
    expect(() => parseArgs(["--artifact"])).toThrow(/bad argument/);
  });

  it("rewrites the Linux ceiling test pins and leaves the rest of the file alone", () => {
    const next = structuredClone(contract);
    next.toolchain.packageVersion = "9.9.9";
    next.ceilings.workspace["event-only"].gzipNormalized = 11111;
    const rewritten = rewriteTestPins(testSource, next);
    expect(rewritten).toContain('toolchain.packageVersion).toBe("9.9.9")');
    expect(rewritten).toContain(
      '"event-only": { raw: ' +
        contract.ceilings.workspace["event-only"].raw +
        ", gzipNormalized: 11111 },",
    );
    const outside = (s) => s.slice(0, s.indexOf('it("records the reviewed Linux x64 ceilings'));
    expect(outside(rewritten)).toBe(outside(testSource));
    expect(rewriteTestPins(testSource, contract)).toBe(testSource);
  });

  it("writes a README section per reason naming the run, source and evidence", () => {
    const {
      contract: next,
      gzipEvidence,
      previousVersion,
    } = rebaselineContract(contract, totalsFrom(contract), evidence, "release-bump");
    const release = readmeSection(next, gzipEvidence, previousVersion, "release-bump");
    expect(release).toContain("## v9.9.9 release baseline");
    expect(release).toContain("actions/runs/1) (job 2)");
    expect(release).toContain("Every raw and `gzipNormalized` total is unchanged");
    expect(release).toContain("No actual gzip total moved.");
    expect(release).toContain(
      "re-pinned separately from a native `island-size-darwin.yml` artifact",
    );

    const legacy = legacyContract(contract);
    const migration = rebaselineContract(
      legacy,
      totalsFrom(contract, {
        "workspace/model": { gzip: legacy.ceilings.workspace.model.gzip + 1 },
      }),
      { ...evidence, version: "4.3.1" },
      "metric-migration",
    );
    const section = readmeSection(
      migration.contract,
      migration.gzipEvidence,
      migration.previousVersion,
      "metric-migration",
    );
    expect(section).toContain("## v4.3.1 metric migration");
    expect(section).toContain("not comparable with any earlier `gzip` ceiling");
    expect(section).toContain(
      "Actual gzip moved on 1 rows by +1 to +1 bytes (workspace model +1); that is transfer-size evidence in the sidecar, not a ceiling.",
    );
    expect(section).toContain("re-verifies these ceilings on the integrated tree in CI");

    const identity = readmeSection(next, [], previousVersion, "runtime-identity");
    expect(identity).toContain("## v9.9.9 runtime-identity re-pin");
    expect(identity).toContain("These totals alone do not prove byte-identical product code");
  });
});
