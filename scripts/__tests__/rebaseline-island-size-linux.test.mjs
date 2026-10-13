import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";

import {
  ARTIFACT_ARGS,
  FINGERPRINT_HELPER_MOVEMENT,
  REASONS,
  actualGzipEvidence,
  artifactCheckoutMismatches,
  assertLockfileOnlyMovement,
  contractDiffKeys,
  describeDeltas,
  diffCeilings,
  main,
  measuredIdentity,
  measuredTotals,
  parseArgs,
  proveFingerprintHelper,
  readmeSection,
  rebaselineContract,
  repinFingerprintHelper,
  rewriteTestPins,
} from "../rebaseline-island-size-linux.mjs";
import {
  asSchemaV2,
  ceilingsOf,
  makeCheckout,
  makeHelperCheckout,
  writeRuns,
} from "./fixtures/island-size-checkout.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const liveContract = JSON.parse(
  readFileSync(join(root, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
);
const contract = asSchemaV2(liveContract);
// The cases below build their own actual-gzip evidence; start from a contract that
// has none, whatever the live contract recorded at its last re-pin.
delete contract.baselineEvidence.actualGzip;
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
    // The previous schema-2 contract recorded no actual gzip, so there is no
    // prior value to compare with: reported as unknown, not as "no movement".
    expect(gzipEvidence).toBeNull();
    expect(contract.toolchain.packageVersion).not.toBe("9.9.9");
  });

  it("refuses a raw change under every reason", () => {
    const totals = totalsFrom(contract, {
      "packed/show-for": { raw: contract.ceilings.packed["show-for"].raw + 1 },
    });
    expect(diffCeilings(contract, totals).rawChanges).toHaveLength(1);
    for (const reason of ["release-bump", "runtime-identity", "lockfile-provenance"]) {
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
    for (const reason of ["release-bump", "runtime-identity", "lockfile-provenance"]) {
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
      /--reason release-bump needs a schemaVersion 2 or 3 contract, found schemaVersion 1/,
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
    expect(Object.keys(REASONS)).toEqual([
      "release-bump",
      "metric-migration",
      "runtime-identity",
      "lockfile-provenance",
      "fingerprint-migration",
      "fingerprint-helper",
    ]);
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
      /--reason must be one of release-bump, metric-migration, runtime-identity, lockfile-provenance, fingerprint-migration, fingerprint-helper$/,
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
    expect(rewriteTestPins(testSource, liveContract)).toBe(testSource);
  });

  it("reads the previous actual gzip of a schemaVersion 2 contract from recorded evidence", () => {
    const totals = totalsFrom(contract);
    expect(actualGzipEvidence(contract, totals)).toBeNull();

    const recorded = structuredClone(contract);
    recorded.baselineEvidence.actualGzip = Object.fromEntries(
      ["workspace", "packed"].map((mode) => [
        mode,
        Object.fromEntries(Object.entries(totals[mode]).map(([name, row]) => [name, row.gzip])),
      ]),
    );
    expect(actualGzipEvidence(recorded, totals)).toEqual([]);
    recorded.baselineEvidence.actualGzip.packed["event-only"] -= 7;
    expect(actualGzipEvidence(recorded, totals)).toEqual([
      { mode: "packed", fixture: "event-only", delta: 7 },
    ]);

    const repinned = rebaselineContract(recorded, totals, evidence, "release-bump");
    expect(repinned.gzipEvidence).toHaveLength(1);
    expect(repinned.contract.baselineEvidence.actualGzip.packed["event-only"]).toBe(
      totals.packed["event-only"].gzip,
    );
    const noEvidence = rebaselineContract(contract, totals, evidence, "release-bump");
    expect(noEvidence.gzipEvidence).toBeNull();
    expect(readmeSection(noEvidence.contract, null, "1.0.0", "release-bump")).toContain(
      "No previous actual gzip evidence was recorded",
    );
  });

  it("writes a README section per reason naming the run, source and evidence", () => {
    const {
      contract: next,
      gzipEvidence,
      previousVersion,
    } = rebaselineContract(contract, totalsFrom(contract), evidence, "release-bump");
    const release = readmeSection(next, gzipEvidence ?? [], previousVersion, "release-bump");
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
  describe("lockfile-provenance (#4184)", () => {
    // A measurement provenance whose identity inputs equal the live contract.
    const provenance = {
      fixtureSha256: { ...contract.fixtureSha256 },
      runnerSha256: { ...contract.runnerSha256 },
      cargoLockSha256: contract.toolchain.cargoLockSha256,
      honoPackageVersion: contract.toolchain.honoVersion,
      esbuildVersion: contract.toolchain.esbuildVersion,
      nodeVersion: contract.toolchain.nodeVersion,
      zlibVersion: contract.toolchain.zlibVersion,
    };
    const lockEvidence = {
      ...evidence,
      version: contract.toolchain.packageVersion,
      normalizerSha256: contract.normalizerSha256,
      pnpmLockSha256: "f".repeat(64),
      runLabel: "Consumer lockfile re-pin run",
      identity: measuredIdentity(provenance),
    };

    it("re-pins only the lock hash at an unchanged producer version", () => {
      const { contract: next, previousVersion } = rebaselineContract(
        contract,
        totalsFrom(contract),
        lockEvidence,
        "lockfile-provenance",
      );
      expect(next.outcome).toBe("lockfile-provenance-repin");
      expect(next.toolchain).toEqual({ ...contract.toolchain, pnpmLockSha256: "f".repeat(64) });
      expect(next.fixtureSha256).toEqual(contract.fixtureSha256);
      expect(next.runnerSha256).toEqual(contract.runnerSha256);
      expect(next.normalizerSha256).toBe(contract.normalizerSha256);
      expect(next.ceilings).toEqual(contract.ceilings);
      expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
      expect(next.platformStatus).toContain("consumer-only pnpm-lock.yaml change");
      expect(next.platformStatus).toContain("This is not a release or runtime change.");
      expect(next.platformStatus).not.toMatch(/release-bump|release baseline|runtime-identity/);
      expect(next.baselineEvidence.note).toContain("Consumer-only pnpm-lock.yaml move");
      const section = readmeSection(next, [], previousVersion, "lockfile-provenance");
      expect(section).toContain(
        "## v" + contract.toolchain.packageVersion + " lockfile-provenance re-pin",
      );
      expect(section).toContain("consumer-only `pnpm-lock.yaml` change");
      expect(section).toContain("this is not a release or runtime change");
      expect(section).not.toContain("release baseline");
    });

    it("rejects a stale or wrong tuple", () => {
      expect(() =>
        rebaselineContract(
          contract,
          totalsFrom(contract),
          { ...lockEvidence, version: "9.9.9" },
          "lockfile-provenance",
        ),
      ).toThrow(/allows only pnpm-lock.yaml to move, but packageVersion differ/);
      expect(() =>
        rebaselineContract(
          contract,
          totalsFrom(contract),
          { ...lockEvidence, pnpmLockSha256: contract.toolchain.pnpmLockSha256 },
          "lockfile-provenance",
        ),
      ).toThrow(/hash unchanged; nothing to re-pin/);
      const { identity: _identity, ...withoutIdentity } = lockEvidence;
      expect(() => assertLockfileOnlyMovement(contract, withoutIdentity)).toThrow(
        /needs the artifact's measured identity inputs/,
      );
    });

    it.each([
      ["normalizerSha256", (e) => ({ ...e, normalizerSha256: "e".repeat(64) })],
      [
        "fixtureSha256",
        (e) => ({
          ...e,
          identity: {
            ...e.identity,
            fixtureSha256: { ...e.identity.fixtureSha256, model: "0".repeat(64) },
          },
        }),
      ],
      [
        "runnerSha256",
        (e) => ({
          ...e,
          identity: {
            ...e.identity,
            runnerSha256: { ...e.identity.runnerSha256, "measure.mjs": "0".repeat(64) },
          },
        }),
      ],
      [
        "cargoLockSha256",
        (e) => ({ ...e, identity: { ...e.identity, cargoLockSha256: "0".repeat(64) } }),
      ],
      [
        "toolchain.nodeVersion",
        (e) => ({
          ...e,
          identity: { ...e.identity, toolchain: { ...e.identity.toolchain, nodeVersion: "v0" } },
        }),
      ],
    ])("rejects an unintended %s movement", (name, edit) => {
      expect(() =>
        rebaselineContract(
          contract,
          totalsFrom(contract),
          edit(lockEvidence),
          "lockfile-provenance",
        ),
      ).toThrow(new RegExp("but " + name.replace(".", "\\.") + " differ"));
    });

    it("leaves the existing reasons' behavior unchanged", () => {
      const { contract: release } = rebaselineContract(
        contract,
        totalsFrom(contract),
        evidence,
        "release-bump",
      );
      expect(release.outcome).toBe("release-version-stamp");
      expect(release.toolchain.packageVersion).toBe("9.9.9");
      const { contract: identity } = rebaselineContract(
        contract,
        totalsFrom(contract),
        evidence,
        "runtime-identity",
      );
      expect(identity.outcome).toBe("runtime-identity-repin");
    });
  });
});

describe("fingerprint-migration (#4187)", () => {
  const roots = [];
  afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    vi.restoreAllMocks();
  });

  // The checkout's full contract-shape fingerprint and the run provenance's
  // {policyVersion, sha256} subset of it.
  const fingerprint = {
    policyVersion: 1,
    parser: "yaml@2.9.0",
    helperSha256: "1".repeat(64),
    sha256: "2".repeat(64),
  };
  const artifactFingerprint = { policyVersion: 1, sha256: fingerprint.sha256 };
  // The runners now record the fingerprint, so their digests moved.
  const newRunners = { "measure-real.mjs": "3".repeat(64), "measure.mjs": "4".repeat(64) };
  const migrationEvidence = {
    ...evidence,
    version: contract.toolchain.packageVersion,
    normalizerSha256: contract.normalizerSha256,
    pnpmLockSha256: "f".repeat(64),
    runLabel: "Dependency-fingerprint migration run",
    identity: measuredIdentity({
      fixtureSha256: { ...contract.fixtureSha256 },
      runnerSha256: newRunners,
      cargoLockSha256: contract.toolchain.cargoLockSha256,
      honoPackageVersion: contract.toolchain.honoVersion,
      esbuildVersion: contract.toolchain.esbuildVersion,
      nodeVersion: contract.toolchain.nodeVersion,
      zlibVersion: contract.toolchain.zlibVersion,
    }),
    artifactFingerprint,
    dependencyFingerprint: fingerprint,
    dependencyFingerprintError: null,
  };
  const migrate = (base = contract, edit = (e) => e, totals = totalsFrom(base)) =>
    rebaselineContract(base, totals, edit(migrationEvidence), "fingerprint-migration");

  it("migrates schemaVersion 2 to 3, moving only the lock pin and runner digests", () => {
    const { contract: next, previousVersion } = migrate();
    expect(Object.keys(next).slice(0, 6)).toEqual([
      "schemaVersion",
      "metric",
      "normalizationVersion",
      "normalizerSha256",
      "dependencyFingerprint",
      "decisionIssue",
    ]);
    expect(next.schemaVersion).toBe(3);
    expect(next.dependencyFingerprint).toEqual(fingerprint);
    expect(Object.keys(next.dependencyFingerprint)).toEqual([
      "policyVersion",
      "parser",
      "helperSha256",
      "sha256",
    ]);
    expect(next.runnerSha256).toEqual(newRunners);
    expect(next.toolchain).toEqual({ ...contract.toolchain, pnpmLockSha256: "f".repeat(64) });
    expect(next.fixtureSha256).toEqual(contract.fixtureSha256);
    expect(next.normalizerSha256).toBe(contract.normalizerSha256);
    expect(next.ceilings).toEqual(contract.ceilings);
    expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(next.outcome).toBe("fingerprint-migration");
    expect(next.baselineSourceSha).toBe(evidence.headSha);
    expect(next.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 1,
      jobId: 2,
      migratedFrom: {
        schemaVersion: 2,
        outcome: contract.outcome,
        baselineSourceSha: contract.baselineSourceSha,
        baselineRunId: contract.baselineEvidence.runId,
      },
    });
    expect(next.baselineEvidence.note).toContain("schemaVersion 2 to 3");
    expect(next.platformStatus).toContain("migrated to schemaVersion 3");
    expect(next.platformStatus).toContain("recorded as baseline provenance only");
    expect(next.platformStatus).not.toMatch(/release-bump|release baseline/);
    // Everything else is carried over untouched.
    const strip = ({
      schemaVersion: _v,
      dependencyFingerprint: _f,
      runnerSha256: _r,
      toolchain: _t,
      outcome: _o,
      baselineSourceSha: _s,
      platformStatus: _p,
      baselineEvidence: _e,
      ...rest
    }) => rest;
    expect(strip(next)).toEqual(strip(contract));

    const section = readmeSection(next, [], previousVersion, "fingerprint-migration");
    expect(section).toContain(
      "## v" + contract.toolchain.packageVersion + " dependency-fingerprint migration",
    );
    expect(section).toContain("[workflow run 1]");
    expect(section).toContain("`" + fingerprint.sha256 + "`");
    expect(section).toContain("`" + "f".repeat(64) + "`");
    expect(section).toContain("LOCK-FINGERPRINT-POLICY.md");
    expect(section).toContain("every raw and `gzipNormalized` total is unchanged");

    const pins = rewriteTestPins(testSource, next);
    expect(pins).toContain("linuxContract.schemaVersion).toBe(3)");
    expect(rewriteTestPins(pins, next)).toBe(pins);
  });

  it("refuses a contract that is not schemaVersion 2", () => {
    const { contract: v3 } = migrate();
    expect(() => migrate(v3)).toThrow(
      /--reason fingerprint-migration needs a schemaVersion 2 contract, found schemaVersion 3/,
    );
    expect(() => migrate(legacyContract(contract))).toThrow(/found schemaVersion 1/);
  });

  it("refuses raw, gzipNormalized and fixture-set movement", () => {
    const raw = totalsFrom(contract, {
      "packed/model": { raw: contract.ceilings.packed.model.raw + 1 },
    });
    expect(() => migrate(contract, undefined, raw)).toThrow(/raw totals changed \(packed\/model\)/);
    const normalized = totalsFrom(contract, {
      "workspace/show-for": {
        gzipNormalized: contract.ceilings.workspace["show-for"].gzipNormalized - 1,
      },
    });
    expect(() => migrate(contract, undefined, normalized)).toThrow(
      /gzipNormalized totals changed \(workspace\/show-for -1\)/,
    );
    const missing = totalsFrom(contract);
    delete missing.packed["json-api"];
    expect(() => migrate(contract, undefined, missing)).toThrow(/fixture set differs/);
  });

  it("refuses an artifact fingerprint that differs from the checkout", () => {
    expect(() =>
      migrate(contract, (e) => ({
        ...e,
        artifactFingerprint: { ...artifactFingerprint, sha256: "0".repeat(64) },
      })),
    ).toThrow(/artifact dependency fingerprint sha256 differs from this checkout's/);
    expect(() =>
      migrate(contract, (e) => ({
        ...e,
        artifactFingerprint: { ...artifactFingerprint, policyVersion: 2 },
      })),
    ).toThrow(/policyVersion differs/);
    expect(() => migrate(contract, (e) => ({ ...e, artifactFingerprint: undefined }))).toThrow(
      /must carry dependencyFingerprint with exactly policyVersion and sha256/,
    );
    expect(() =>
      migrate(contract, (e) => ({
        ...e,
        artifactFingerprint: { ...artifactFingerprint, parser: "yaml@2.9.0" },
      })),
    ).toThrow(/with exactly policyVersion and sha256/);
    expect(() =>
      migrate(contract, (e) => ({
        ...e,
        dependencyFingerprint: null,
        dependencyFingerprintError: "UNKNOWN_KEY: x",
      })),
    ).toThrow(/checkout dependency fingerprint failed: UNKNOWN_KEY: x/);
  });

  it.each([
    ["packageVersion", (e) => ({ ...e, version: "9.9.9" })],
    ["normalizerSha256", (e) => ({ ...e, normalizerSha256: "e".repeat(64) })],
    [
      "fixtureSha256",
      (e) => ({
        ...e,
        identity: {
          ...e.identity,
          fixtureSha256: { ...e.identity.fixtureSha256, model: "0".repeat(64) },
        },
      }),
    ],
    [
      "cargoLockSha256",
      (e) => ({ ...e, identity: { ...e.identity, cargoLockSha256: "0".repeat(64) } }),
    ],
    [
      "toolchain.esbuildVersion",
      (e) => ({
        ...e,
        identity: { ...e.identity, toolchain: { ...e.identity.toolchain, esbuildVersion: "0" } },
      }),
    ],
  ])("refuses an unintended %s movement", (name, edit) => {
    expect(() => migrate(contract, edit)).toThrow(
      new RegExp("changes only the lock pin, but " + name.replace(".", "\\.") + " differ"),
    );
  });

  it("keeps lockfile-provenance on schemaVersion 2 only", () => {
    const { contract: v3 } = migrate();
    expect(() =>
      rebaselineContract(v3, totalsFrom(v3), migrationEvidence, "lockfile-provenance"),
    ).toThrow(
      /--reason lockfile-provenance needs a schemaVersion 2 contract, found schemaVersion 3/,
    );
  });

  for (const reason of ["release-bump", "runtime-identity"]) {
    it(reason + " re-pins a schemaVersion 3 contract and never moves its fingerprint", () => {
      const { contract: v3 } = migrate();
      const bump = { ...migrationEvidence, version: "9.9.9", pnpmLockSha256: "9".repeat(64) };
      const { contract: next } = rebaselineContract(v3, totalsFrom(v3), bump, reason);
      expect(next.schemaVersion).toBe(3);
      expect(next.dependencyFingerprint).toEqual(v3.dependencyFingerprint);
      expect(next.runnerSha256).toEqual(v3.runnerSha256);
      expect(next.toolchain.packageVersion).toBe("9.9.9");
      expect(next.toolchain.pnpmLockSha256).toBe("9".repeat(64));
      expect(next.ceilings).toEqual(v3.ceilings);

      // The checkout's graph moved: the artifact agrees with it, the contract does not.
      const moved = {
        ...bump,
        artifactFingerprint: { policyVersion: 1, sha256: "8".repeat(64) },
        dependencyFingerprint: { ...fingerprint, sha256: "8".repeat(64) },
      };
      expect(() => rebaselineContract(v3, totalsFrom(v3), moved, reason)).toThrow(
        new RegExp(
          "--reason " + reason + " cannot move the dependency fingerprint \\(sha256 differ",
        ),
      );
      const helper = {
        ...bump,
        dependencyFingerprint: { ...fingerprint, helperSha256: "7".repeat(64) },
      };
      expect(() => rebaselineContract(v3, totalsFrom(v3), helper, reason)).toThrow(
        /cannot move the dependency fingerprint \(helperSha256 differ/,
      );
      expect(() =>
        rebaselineContract(v3, totalsFrom(v3), { ...bump, artifactFingerprint: undefined }, reason),
      ).toThrow(/must carry dependencyFingerprint/);
    });
  }

  it("compares the artifact with the checkout byte-for-byte, fingerprint included", () => {
    const checkout = {
      packageVersion: "4.4.0",
      pnpmLockSha256: "a".repeat(64),
      cargoLockSha256: "b".repeat(64),
      fixtureSha256: { ...contract.fixtureSha256 },
      runnerSha256: newRunners,
      dependencyFingerprint: fingerprint,
      dependencyFingerprintError: null,
    };
    const provenance = {
      packageVersion: "4.4.0",
      pnpmLockSha256: "a".repeat(64),
      cargoLockSha256: "b".repeat(64),
      fixtureSha256: { ...contract.fixtureSha256 },
      runnerSha256: { ...newRunners },
      dependencyFingerprint: { ...artifactFingerprint },
    };
    expect(artifactCheckoutMismatches(provenance, checkout, "packed")).toEqual([]);
    const errors = artifactCheckoutMismatches(
      {
        ...provenance,
        packageVersion: "4.4.1",
        pnpmLockSha256: "c".repeat(64),
        cargoLockSha256: "d".repeat(64),
        fixtureSha256: { ...provenance.fixtureSha256, model: "0".repeat(64) },
        runnerSha256: contract.runnerSha256,
        dependencyFingerprint: { ...artifactFingerprint, sha256: "0".repeat(64) },
      },
      checkout,
      "packed",
    );
    expect(errors).toEqual([
      "packed artifact lockfile hash differs from this checkout's pnpm-lock.yaml",
      "packed artifact measured @takazudo/zfb 4.4.1, not 4.4.0",
      "packed artifact Cargo.lock hash differs from this checkout's Cargo.lock",
      "packed artifact fixtureSha256 differs from this checkout's files",
      "packed artifact runnerSha256 differs from this checkout's files",
      "packed artifact dependency fingerprint sha256 differs from this checkout's",
    ]);
  });

  // An artifact shaped like the merged producers' output: provenance carries the
  // fingerprint, and its check-budget run against the v2 contract failed.
  function nativeLinuxArtifact(checkout, edit = (p) => p) {
    const artifact = join(checkout.root, "artifact");
    writeRuns(artifact, Object.keys(contract.ceilings.workspace), (mode) =>
      edit({
        sourceSha: "b".repeat(40),
        mode,
        platform: { os: "linux", arch: "x64" },
        fixtureVersion: contract.fixtureVersion,
        fixtureSha256: checkout.fixtureSha256,
        runnerSha256: checkout.runnerSha256,
        packageVersion: checkout.version,
        runtimePackageVersion: checkout.version,
        honoPackageVersion: contract.toolchain.honoVersion,
        esbuildVersion: contract.toolchain.esbuildVersion,
        nodeVersion: contract.toolchain.nodeVersion,
        zlibVersion: contract.toolchain.zlibVersion,
        compression: contract.compression,
        repeats: 2,
        pnpmLockSha256: checkout.pnpmLockSha256,
        dependencyFingerprint: {
          policyVersion: checkout.dependencyFingerprint.policyVersion,
          sha256: checkout.dependencyFingerprint.sha256,
        },
        cargoLockSha256: checkout.cargoLockSha256,
      }),
    );
    writeFileSync(join(artifact, "check-budget.exit-code"), "1\n");
    writeFileSync(
      join(artifact, "check-budget.log"),
      "packed provenance: schema mismatch: schemaVersion 2 contract cannot accept a run carrying dependencyFingerprint\n",
    );
    // The live-shaped v2 contract: stale runner digests and whole-file lock hash.
    const v2 = structuredClone(contract);
    v2.ceilings = ceilingsOf(artifact);
    v2.fixtureSha256 = { ...checkout.fixtureSha256 };
    v2.toolchain.packageVersion = checkout.version;
    v2.toolchain.cargoLockSha256 = checkout.cargoLockSha256;
    v2.toolchain.pnpmLockSha256 = "5".repeat(64);
    v2.runnerSha256 = { "measure-real.mjs": "6".repeat(64), "measure.mjs": "7".repeat(64) };
    writeFileSync(
      join(checkout.root, "research/v3-island-size/decision-linux-x64.json"),
      JSON.stringify(v2, null, 2) + "\n",
    );
    return { artifact, v2 };
  }

  const cli = (artifact, reason = "fingerprint-migration") => [
    "--artifact",
    artifact,
    "--run-id",
    "41",
    "--job-id",
    "42",
    "--head-sha",
    "c".repeat(40),
    "--reason",
    reason,
  ];

  it("runs end-to-end on a native artifact whose v2 check-budget run failed", () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    const checkout = makeCheckout(roots);
    const { artifact, v2 } = nativeLinuxArtifact(checkout);
    main(cli(artifact), checkout.root);
    const written = JSON.parse(
      readFileSync(join(checkout.root, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
    );
    expect(written.schemaVersion).toBe(3);
    expect(written.dependencyFingerprint).toEqual(checkout.dependencyFingerprint);
    expect(written.runnerSha256).toEqual(checkout.runnerSha256);
    expect(written.toolchain).toEqual({ ...v2.toolchain, pnpmLockSha256: checkout.pnpmLockSha256 });
    expect(written.ceilings).toEqual(v2.ceilings);
    expect(written.baselineEvidence).toMatchObject({
      runId: 41,
      jobId: 42,
      sourceSha: "b".repeat(40),
    });
    const pins = readFileSync(
      join(checkout.root, "scripts/__tests__/island-size-budget.test.mjs"),
      "utf8",
    );
    expect(pins).toContain("linuxContract.schemaVersion).toBe(3)");
    const readme = readFileSync(join(checkout.root, "research/v3-island-size/README.md"), "utf8");
    expect(readme).toContain("dependency-fingerprint migration");
    expect(readme).toContain(checkout.dependencyFingerprint.sha256);
  });

  it("refuses end-to-end when the artifact fingerprint is not the checkout's", () => {
    const checkout = makeCheckout(roots);
    const { artifact } = nativeLinuxArtifact(checkout, (p) => ({
      ...p,
      dependencyFingerprint: { ...p.dependencyFingerprint, sha256: "0".repeat(64) },
    }));
    expect(() => main(cli(artifact), checkout.root)).toThrow(
      /workspace artifact dependency fingerprint sha256 differs from this checkout's/,
    );
  });

  it("refuses end-to-end when the artifact's runners are not the checkout's", () => {
    const stale = makeCheckout(roots);
    const { artifact: oldRunners } = nativeLinuxArtifact(stale, (p) => ({
      ...p,
      runnerSha256: { ...p.runnerSha256, "measure.mjs": "0".repeat(64) },
    }));
    expect(() => main(cli(oldRunners), stale.root)).toThrow(
      /workspace artifact runnerSha256 differs from this checkout's files/,
    );
  });
});

describe("fingerprint-helper (#4198)", () => {
  const roots = [];
  afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    vi.restoreAllMocks();
  });

  const PINNED = "1".repeat(64);
  const NEW = "2".repeat(64);
  const fingerprint = {
    policyVersion: 1,
    parser: "yaml@2.9.0",
    helperSha256: PINNED,
    sha256: "3".repeat(64),
  };
  const darwinLive = JSON.parse(
    readFileSync(join(root, "research/v3-island-size/decision.json"), "utf8"),
  );
  const pinnedTo = (base) => ({
    ...structuredClone(base),
    dependencyFingerprint: { ...fingerprint },
    outcome: "release-version-stamp",
  });
  const contracts = () => ({ linux: pinnedTo(liveContract), darwin: pinnedTo(darwinLive) });
  const inventory = {
    importers: ["packages/zfb"],
    snapshots: ["hono@4.12.25"],
    packages: ["hono@4.12.25"],
    sections: { environment: ["importers"], project: ["importers"] },
    counts: {
      importers: { reached: 1, total: 2 },
      snapshots: { reached: 1, total: 3 },
      packages: { reached: 1, total: 3 },
    },
  };
  const helperResult = (helperSha256) => ({
    policyVersion: 1,
    parser: "yaml@2.9.0",
    helperSha256,
    sha256: fingerprint.sha256,
    inventory: structuredClone(inventory),
  });
  const proof = (edit = (p) => p) =>
    edit({
      baseRef: "base/x",
      baseCommit: "c".repeat(40),
      baseHelperSha256: PINNED,
      newHelperSha256: NEW,
      current: helperResult(NEW),
      base: helperResult(PINNED),
    });

  it("moves only helperSha256, outcome and platformStatus of both contracts", () => {
    const before = contracts();
    const { contracts: after, previousHelperSha256 } = repinFingerprintHelper(before, proof());
    expect(previousHelperSha256).toBe(PINNED);
    for (const name of ["linux", "darwin"]) {
      expect(contractDiffKeys(before[name], after[name])).toEqual([
        "dependencyFingerprint.helperSha256",
        "outcome",
        "platformStatus",
      ]);
      expect(after[name].dependencyFingerprint).toEqual({ ...fingerprint, helperSha256: NEW });
      expect(after[name].outcome).toBe("fingerprint-helper-repin");
      expect(after[name].platformStatus).toBe(
        before[name].platformStatus +
          " Helper re-pinned from 11111111 to 22222222 under #4198 at base/x; measured inputs and fingerprint unchanged.",
      );
      for (const key of [
        "ceilings",
        "allowance",
        "baselineEvidence",
        "toolchain",
        "runnerSha256",
      ]) {
        expect(JSON.stringify(after[name][key])).toBe(JSON.stringify(before[name][key]));
      }
      expect(Object.keys(after[name])).toEqual(Object.keys(before[name]));
    }
    expect(FINGERPRINT_HELPER_MOVEMENT).toEqual([
      "dependencyFingerprint.helperSha256",
      "outcome",
      "platformStatus",
    ]);
  });

  it.each([
    ["normalizerSha256", (c) => (c.normalizerSha256 = "0".repeat(64))],
    ["fixtureSha256", (c) => (c.fixtureSha256.model = "0".repeat(64))],
    ["runnerSha256", (c) => (c.runnerSha256["measure.mjs"] = "0".repeat(64))],
    ["toolchain.pnpmLockSha256", (c) => (c.toolchain.pnpmLockSha256 = "0".repeat(64))],
    ["toolchain.cargoLockSha256", (c) => (c.toolchain.cargoLockSha256 = "0".repeat(64))],
    ["toolchain.packageVersion", (c) => (c.toolchain.packageVersion = "0.0.0")],
    ["baselineSourceSha", (c) => (c.baselineSourceSha = "0".repeat(40))],
    ["a ceiling", (c) => (c.ceilings.packed.model.raw += 1)],
  ])("never writes %s: an identity input is carried over byte for byte", (_name, edit) => {
    const before = contracts();
    edit(before.linux);
    edit(before.darwin);
    const { contracts: after } = repinFingerprintHelper(before, proof());
    for (const name of ["linux", "darwin"]) {
      expect(contractDiffKeys(before[name], after[name])).toEqual(FINGERPRINT_HELPER_MOVEMENT);
    }
  });

  it.each([
    [
      "sha256 moved",
      (c) => (c.linux.dependencyFingerprint.sha256 = "4".repeat(64)),
      (p) => p,
      /fingerprint value moved \(research\/v3-island-size\/decision-linux-x64\.json pins 4{64}.*`dependency-graph` \(#4218\), a policy change needs `fingerprint-policy` \(#4218\)/,
    ],
    [
      "policyVersion moved",
      (c) => c,
      (p) => ({ ...p, current: { ...p.current, policyVersion: 2 } }),
      /policy identity moved \(policyVersion of .*`fingerprint-policy` change \(#4218\), not a helper re-pin/,
    ],
    [
      "parser moved",
      (c) => c,
      (p) => ({ ...p, current: { ...p.current, parser: "yaml@2.9.1" } }),
      /policy identity moved \(parser of .*`fingerprint-policy`/,
    ],
    [
      "helper and sha256 moved together",
      (c) => c,
      (p) => ({ ...p, current: { ...p.current, sha256: "5".repeat(64) } }),
      /fingerprint value moved .*`fingerprint-policy` \(#4218\)/,
    ],
    [
      "a schemaVersion 2 contract",
      (c) => (c.darwin = asSchemaV2(c.darwin)),
      (p) => p,
      /needs a schemaVersion 3 contract, found 2 in research\/v3-island-size\/decision\.json; migrate with `fingerprint-migration` first/,
    ],
    [
      "an unchanged helper",
      (c) => c,
      (p) => ({ ...p, newHelperSha256: PINNED, current: helperResult(PINNED) }),
      /helper unchanged; nothing to re-pin/,
    ],
    [
      "a base-ref helper that is not the pin",
      (c) => c,
      (p) => ({ ...p, baseHelperSha256: "6".repeat(64) }),
      /the helper at base\/x \(6{64}\) is not the pinned helper \(1{64}\); pass the ref whose helper the contract pins/,
    ],
    [
      "an old/new inventory difference",
      (c) => c,
      (p) => {
        p.base.inventory.snapshots.push("yaml@2.9.0");
        return p;
      },
      /old and new helper disagree on the real lock \(inventory\.snapshots\); this is a `fingerprint-policy` change \(#4218\)/,
    ],
    [
      "an old/new count difference",
      (c) => c,
      (p) => {
        p.current.inventory.counts.packages.total += 1;
        return p;
      },
      /disagree on the real lock \(inventory\.counts\)/,
    ],
    [
      "an old/new sha256 difference",
      (c) => c,
      (p) => ({ ...p, base: { ...p.base, sha256: "7".repeat(64) } }),
      /old and new helper disagree on the real lock \(sha256\); this is a `fingerprint-policy` change/,
    ],
    [
      "a pinned helper that fails on the real lock",
      (c) => c,
      (p) => ({ ...p, base: { error: "UNKNOWN_KEY: x" } }),
      /disagree on the real lock \(the pinned helper failed: UNKNOWN_KEY: x\)/,
    ],
    [
      "contracts that disagree on helperSha256",
      (c) => (c.darwin.dependencyFingerprint.helperSha256 = "8".repeat(64)),
      (p) => p,
      /contracts disagree on helperSha256 \(.*\); fix the contract that is wrong first/,
    ],
    [
      "one platform's contract only",
      (c) => delete c.darwin,
      (p) => p,
      /missing the research\/v3-island-size\/decision\.json contract/,
    ],
  ])("refuses %s", (_name, editContracts, editProof, message) => {
    const before = contracts();
    editContracts(before);
    expect(() => repinFingerprintHelper(before, proof(editProof))).toThrow(message);
  });

  it("refuses every artifact argument and takes only --base-ref", () => {
    const ok = ["--reason", "fingerprint-helper", "--base-ref", "main"];
    expect(parseArgs(ok)).toEqual({ reason: "fingerprint-helper", "base-ref": "main" });
    for (const key of ARTIFACT_ARGS) {
      expect(() => parseArgs([...ok, "--" + key, "x"])).toThrow(
        "--reason fingerprint-helper takes no artifact (no native run); remove --" + key,
      );
    }
    for (const key of ["force", "ignore-provenance", "contract"]) {
      expect(() => parseArgs([...ok, "--" + key, "1"])).toThrow(
        "--reason fingerprint-helper takes only --base-ref; remove --" + key,
      );
    }
    expect(() => parseArgs(ok.slice(0, 2))).toThrow("required --base-ref");
    expect(() =>
      parseArgs([
        "--artifact",
        "dir",
        "--run-id",
        "1",
        "--job-id",
        "2",
        "--head-sha",
        "d".repeat(40),
        "--reason",
        "release-bump",
        "--base-ref",
        "main",
      ]),
    ).toThrow("--base-ref is only for --reason fingerprint-helper");
    expect(() => rebaselineContract(liveContract, {}, evidence, "fingerprint-helper")).toThrow(
      /takes no artifact/,
    );
  });

  it("re-pins both contracts end to end after a comment-only helper edit", async () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    const repo = makeHelperCheckout(roots);
    appendFileSync(repo.helperPath, "// maintenance comment\n");
    const newHelper = createHash("sha256").update(readFileSync(repo.helperPath)).digest("hex");
    await main(["--reason", "fingerprint-helper", "--base-ref", "HEAD"], repo.root);
    const read = (file) =>
      JSON.parse(readFileSync(join(repo.root, "research/v3-island-size", file), "utf8"));
    for (const [name, file] of [
      ["linux", "decision-linux-x64.json"],
      ["darwin", "decision.json"],
    ]) {
      const written = read(file);
      expect(contractDiffKeys(repo.contracts[name], written)).toEqual(FINGERPRINT_HELPER_MOVEMENT);
      expect(written.dependencyFingerprint).toEqual({
        ...repo.dependencyFingerprint,
        helperSha256: newHelper,
      });
    }
    // The CLI writes `--explain` output; its exact bytes are guarded on the real
    // repository by island-size-lock-fingerprint.test.mjs.
    const inventoryFile = JSON.parse(
      readFileSync(
        join(repo.root, "research/v3-island-size/lock-fingerprint-inventory.json"),
        "utf8",
      ),
    );
    expect(inventoryFile).toEqual({
      ...repo.dependencyFingerprint,
      helperSha256: newHelper,
      inventory: inventoryFile.inventory,
    });
    expect(inventoryFile.inventory.counts.importers.reached).toBe(8);
    const readme = readFileSync(join(repo.root, "research/v3-island-size/README.md"), "utf8");
    expect(readme).toContain("fingerprint-helper re-pin");
    expect(readme).toContain("`" + newHelper + "`");
    const printed = log.mock.calls.flat().join("\n");
    for (const fragment of [
      "base-ref: HEAD (",
      "old helperSha256: " + repo.dependencyFingerprint.helperSha256,
      "new helperSha256: " + newHelper,
      "sha256 (unchanged): " + repo.dependencyFingerprint.sha256,
      "policyVersion: 1",
      "parser: yaml@2.9.0",
      "inventory (identical for both helpers): importers 8/",
      "no native run: helperSha256 is outside the hashed input and absent from run provenance (#4198)",
    ]) {
      expect(printed).toContain(fragment);
    }
    // A second run finds the new helper already pinned.
    await expect(
      main(["--reason", "fingerprint-helper", "--base-ref", "HEAD"], repo.root),
    ).rejects.toThrow(/helper unchanged; nothing to re-pin/);
  });

  it("refuses end to end a helper edit that moves the fingerprint", async () => {
    const repo = makeHelperCheckout(roots);
    const source = readFileSync(repo.helperPath, "utf8");
    writeFileSync(
      repo.helperPath,
      source.replace("sha256: sha256(canonical),", "sha256: sha256(canonical + ' '),"),
    );
    await expect(
      main(["--reason", "fingerprint-helper", "--base-ref", "HEAD"], repo.root),
    ).rejects.toThrow(/fingerprint value moved .*`fingerprint-policy` \(#4218\)/);
    const unchanged = readFileSync(
      join(repo.root, "research/v3-island-size/decision-linux-x64.json"),
      "utf8",
    );
    expect(JSON.parse(unchanged)).toEqual(repo.contracts.linux);
  });

  it("refuses end to end a base-ref whose helper is not the pinned one", async () => {
    const repo = makeHelperCheckout(roots);
    appendFileSync(repo.helperPath, "// first edit\n");
    repo.git("commit", "-qam", "unpinned edit");
    appendFileSync(repo.helperPath, "// second edit\n");
    await expect(
      main(["--reason", "fingerprint-helper", "--base-ref", "HEAD"], repo.root),
    ).rejects.toThrow(/the helper at HEAD .* is not the pinned helper/);
    await expect(proveFingerprintHelper("no-such-ref", repo.root)).rejects.toThrow(
      /cannot read research\/v3-island-size\/lock-fingerprint\.mjs at --base-ref no-such-ref/,
    );
  });

  it("surfaces a checkout fingerprint error verbatim and writes nothing", async () => {
    const repo = makeHelperCheckout(roots);
    appendFileSync(repo.helperPath, "// comment\n");
    const lock = join(repo.root, "pnpm-lock.yaml");
    writeFileSync(
      lock,
      readFileSync(lock, "utf8").replace("lockfileVersion: '9.0'", "lockfileVersion: '8.0'"),
    );
    await expect(
      main(["--reason", "fingerprint-helper", "--base-ref", "HEAD"], repo.root),
    ).rejects.toMatchObject({
      name: "LockFingerprintError",
      code: "LOCKFILE_VERSION",
      message: "LOCKFILE_VERSION: environment lockfileVersion must be the string '9.0'",
    });
    expect(
      JSON.parse(readFileSync(join(repo.root, "research/v3-island-size/decision.json"), "utf8")),
    ).toEqual(repo.contracts.darwin);
  });
});
