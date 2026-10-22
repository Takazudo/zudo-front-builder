import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";

import {
  ARTIFACT_ARGS,
  CARGO_INTERNAL_REWIRE_MOVEMENT,
  CARGO_PRODUCER_LOCK_MOVEMENT,
  DEPENDENCY_GRAPH_MOVEMENT,
  FINGERPRINT_HELPER_MOVEMENT,
  FINGERPRINT_POLICY_MOVEMENT,
  INVENTORY_REASONS,
  PAIRED_REASONS,
  REASONS,
  REWIRE_ARGS,
  SOURCE_BOUND_REASONS,
  actualGzipEvidence,
  applyStage,
  artifactCheckoutMismatches,
  assertAllowedMovement,
  assertLockfileOnlyMovement,
  checkRewireDeclaration,
  contractDiffKeys,
  describeDeltas,
  diffCargoLocks,
  diffCeilings,
  fingerprintPolicyRefusal,
  helperExplainBytes,
  main,
  measuredIdentity,
  measuredTotals,
  newLockReference,
  parseAddedPackages,
  parseArgs,
  parseCargoLock,
  parseLockRecord,
  parseOldCargoRef,
  predecessorRecord,
  proveCargoInternalRewire,
  proveFingerprintHelper,
  readManifestPackage,
  readmeSection,
  rebaselineContract,
  repinFingerprintHelper,
  resolveDependencyEntry,
  rewritePolicyIdentity,
  lockSourceValue,
  rewriteTestPins,
  workspaceMemberProblem,
} from "../rebaseline-island-size-linux.mjs";
import {
  DARWIN_TARGET,
  LINUX_TARGET,
  REWIRE_MANIFESTS,
  closureReceipt,
  recordOf,
  rewireDeclaration,
  rewireLocks,
  rewireSha,
  writeRewireInputs,
  writeRewireManifests,
} from "./fixtures/island-size-cargo-rewire.mjs";
import {
  addAmbiguousCommitPrefix,
  asSchemaV2,
  ceilingsOf,
  gitIn,
  landStaged,
  makeCheckout,
  makeGitCheckout,
  makeGraphCheckout,
  makeHelperCheckout,
  makePolicyCheckout,
  policyHelper,
  withCurrentNormalizer,
  writeRuns,
} from "./fixtures/island-size-checkout.mjs";
import { moveReachableSnapshot } from "./fixtures/island-size-lock-mutations.mjs";
import { FINGERPRINT_POLICY_VERSION as LIVE_POLICY_VERSION } from "../../research/v3-island-size/lock-fingerprint.mjs";
import { reviewedCargoLock } from "./fixtures/reviewed-cargo-transition.mjs";

// Synchronous git/helper end-to-end cases peak near 5.3 s on hosted runners (median up to 3.7 s); measured in #4364.
vi.setConfig({ testTimeout: 15_000 });

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const liveContract = JSON.parse(
  readFileSync(join(root, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
);
const contract = asSchemaV2(liveContract);
// Verbatim released v4.5.3 Linux contract (main d70dd6d9): a genuine contract
// that predates any cargo-internal-rewire, now that the live one carries one.
const preRewireContract = JSON.parse(
  readFileSync(
    join(root, "scripts/__tests__/fixtures/island-size-linux-x64-v4.5.3-release.json"),
    "utf8",
  ),
);
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
  // release-bump / runtime-identity compare the measured Cargo.lock with the contract.
  identity: { cargoLockSha256: contract.toolchain.cargoLockSha256 },
  // Test fixture: a digest-only previous-contract reference (no git checkout).
  previousContract: { sha256: "d".repeat(64) },
};
// The flat immediate-predecessor shape both platforms record.
const REPINNED_FROM_KEYS = [
  "schemaVersion",
  "outcome",
  "packageVersion",
  "baselineSourceSha",
  "baselineRunId",
  "contract",
];

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
    expect(next.normalizationVersion).toBe(2);
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
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 2,
      outcome: contract.outcome,
      packageVersion: contract.toolchain.packageVersion,
      baselineSourceSha: contract.baselineSourceSha,
      baselineRunId: contract.baselineEvidence.runId,
      contract: { sha256: "d".repeat(64) },
    });
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

  // #4245 finding 2: when health.yml measured a PR merge source distinct from
  // the probe head, the merge source is the only measured provenance and the
  // head keeps baselineSourceSha's historical meaning (the probe head).
  it("keeps the measured merge source verbatim when it differs from the probe head", () => {
    expect(evidence.sourceSha).not.toBe(evidence.headSha);
    const { contract: next } = rebaselineContract(
      contract,
      totalsFrom(contract),
      evidence,
      "release-bump",
    );
    expect(next.baselineSourceSha).toBe(evidence.headSha);
    expect(next.baselineEvidence.sourceSha).toBe(evidence.sourceSha);
    const cited =
      "PR merge source " + evidence.sourceSha + " (probe head " + evidence.headSha + ")";
    const readme = readmeSection(next, [], contract.toolchain.packageVersion, "release-bump");
    expect(readme).toContain(
      "PR merge source `" + evidence.sourceSha + "` (probe head `" + evidence.headSha + "`)",
    );
    expect(next.baselineEvidence.note).toContain(cited);
    expect(next.platformStatus).toContain(cited);
    // The head is only ever cited as the probe head, never as the measured source.
    for (const text of [readme, next.baselineEvidence.note, next.platformStatus]) {
      const plain = text.replaceAll("`", "");
      expect(plain.split(evidence.headSha).length - 1).toBe(
        plain.split("probe head " + evidence.headSha).length - 1,
      );
    }
    // A later re-pin cites this one's probe head as its predecessor, unchanged.
    const later = { ...evidence, sourceSha: "f".repeat(40), headSha: "e".repeat(40), runId: 3 };
    const { contract: after } = rebaselineContract(next, totalsFrom(next), later, "release-bump");
    expect(after.baselineEvidence.repinnedFrom.baselineSourceSha).toBe(evidence.headSha);
    expect(after.baselineEvidence.repinnedFrom.baselineRunId).toBe(evidence.runId);
    expect(after.baselineEvidence.sourceSha).toBe(later.sourceSha);
  });

  it("matches the live Linux contract's own head/source record", () => {
    const { baselineSourceSha, baselineEvidence } = liveContract;
    expect(baselineEvidence.note).toContain(
      baselineEvidence.sourceSha === baselineSourceSha
        ? "source " + baselineSourceSha
        : "PR merge source " +
            baselineEvidence.sourceSha +
            " (probe head " +
            baselineSourceSha +
            ")",
    );
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
    expect(next.baselineEvidence).not.toHaveProperty("repinnedFrom");
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
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(Object.keys(next.baselineEvidence.repinnedFrom.contract)).toEqual(["sha256"]);
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
      "cargo-producer-lock",
      "dependency-graph",
      "fingerprint-policy",
      "cargo-internal-rewire",
      "cargo-removal",
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
      /--reason must be one of release-bump, metric-migration, runtime-identity, lockfile-provenance, fingerprint-migration, fingerprint-helper, cargo-producer-lock, dependency-graph, fingerprint-policy, cargo-internal-rewire, cargo-removal$/,
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
      expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
      expect(next.baselineEvidence.repinnedFrom.outcome).toBe(contract.outcome);
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
    expect(next.baselineEvidence).not.toHaveProperty("repinnedFrom");
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
      // The refusal names the reasons that own each fingerprint key (#4218).
      expect(() => rebaselineContract(v3, totalsFrom(v3), moved, reason)).toThrow(
        /a reachable dependency change needs `--reason dependency-graph` \(#4218\) and a policy or parser change needs `fingerprint-policy` \(#4218\), not a /,
      );
      // dependency-graph's allowlisted keys stay refused under this reason.
      for (const key of DEPENDENCY_GRAPH_MOVEMENT.filter((k) => k.startsWith("dependency"))) {
        const [, field] = key.split(".");
        const edited = {
          ...bump,
          artifactFingerprint: { policyVersion: 1, sha256: "8".repeat(64) },
          dependencyFingerprint: { ...fingerprint, [field]: "8".repeat(64) },
        };
        expect(() => rebaselineContract(v3, totalsFrom(v3), edited, reason)).toThrow(
          "cannot move the dependency fingerprint (" + field + " differ",
        );
      }
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

  it("runs end-to-end on a native artifact whose v2 check-budget run failed", async () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    const checkout = makeCheckout(roots);
    const { artifact, v2 } = nativeLinuxArtifact(checkout);
    await main(cli(artifact), checkout.root);
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

  it("refuses end-to-end when the artifact fingerprint is not the checkout's", async () => {
    const checkout = makeCheckout(roots);
    const { artifact } = nativeLinuxArtifact(checkout, (p) => ({
      ...p,
      dependencyFingerprint: { ...p.dependencyFingerprint, sha256: "0".repeat(64) },
    }));
    await expect(main(cli(artifact), checkout.root)).rejects.toThrow(
      /workspace artifact dependency fingerprint sha256 differs from this checkout's/,
    );
  });

  it("refuses end-to-end when the artifact's runners are not the checkout's", async () => {
    const stale = makeCheckout(roots);
    const { artifact: oldRunners } = nativeLinuxArtifact(stale, (p) => ({
      ...p,
      runnerSha256: { ...p.runnerSha256, "measure.mjs": "0".repeat(64) },
    }));
    await expect(main(cli(oldRunners), stale.root)).rejects.toThrow(
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

describe("cargo-producer-lock (#4214)", () => {
  const roots = [];
  afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    vi.restoreAllMocks();
  });

  const sha = (text) => createHash("sha256").update(text).digest("hex");
  const lockDir = join(root, "scripts/__tests__/fixtures/cargo-producer-lock");
  const oldText = readFileSync(join(lockDir, "old.lock"), "utf8");
  const newText = readFileSync(join(lockDir, "new.lock"), "utf8");
  const OLD = sha(oldText);
  const NEW = sha(newText);
  const ADDED = ["ctor@0.2.9", "zfb-css-node@0.0.0"];
  // Test fixture: the verified git source of the old lock (readCargoLockEvidence).
  const SOURCE = { commit: "a".repeat(40), path: "Cargo.lock", sha256: OLD };
  const v3 = structuredClone(liveContract);
  v3.toolchain.cargoLockSha256 = OLD;
  const fingerprint = v3.dependencyFingerprint;
  const cargoEvidence = {
    ...evidence,
    version: v3.toolchain.packageVersion,
    normalizerSha256: v3.normalizerSha256,
    pnpmLockSha256: v3.toolchain.pnpmLockSha256,
    runLabel: "Cargo producer-lock transition run",
    identity: measuredIdentity({
      fixtureSha256: { ...v3.fixtureSha256 },
      runnerSha256: { ...v3.runnerSha256 },
      cargoLockSha256: NEW,
      honoPackageVersion: v3.toolchain.honoVersion,
      esbuildVersion: v3.toolchain.esbuildVersion,
      nodeVersion: v3.toolchain.nodeVersion,
      zlibVersion: v3.toolchain.zlibVersion,
    }),
    artifactFingerprint: { policyVersion: fingerprint.policyVersion, sha256: fingerprint.sha256 },
    dependencyFingerprint: { ...fingerprint },
    dependencyFingerprintError: null,
    cargoLock: {
      expectedOldSha: OLD,
      expectedNewSha: NEW,
      oldText,
      newText,
      added: ADDED,
      oldLockSource: SOURCE,
    },
  };
  const transition = (edit = (e) => e, base = v3, totals = totalsFrom(base)) =>
    rebaselineContract(base, totals, edit(cargoEvidence), "cargo-producer-lock");
  // A new lock the checkout and the artifact both carry.
  const withNew =
    (text, added = ADDED) =>
    (e) => ({
      ...e,
      identity: { ...e.identity, cargoLockSha256: sha(text) },
      cargoLock: { ...e.cargoLock, newText: text, expectedNewSha: sha(text), added },
    });

  it("records the additive transition, moving only the Cargo hash and its evidence", () => {
    const { contract: next, previousVersion, gzipEvidence } = transition();
    expect(next.outcome).toBe("cargo-producer-lock-transition");
    expect(next.toolchain).toEqual({ ...v3.toolchain, cargoLockSha256: NEW });
    expect(next.ceilings).toEqual(v3.ceilings);
    expect(next.dependencyFingerprint).toEqual(v3.dependencyFingerprint);
    expect(next.baselineEvidence.cargoTransition).toEqual({
      oldSha: OLD,
      newSha: NEW,
      added: ADDED,
      changed: [],
      removed: [],
      oldLockSource: SOURCE,
    });
    expect(Object.keys(next.baselineEvidence)).toEqual([
      "event",
      "runId",
      "jobId",
      "sourceSha",
      "repinnedFrom",
      "cargoTransition",
      "actualGzip",
      "note",
    ]);
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.platformStatus).toContain(
      "proved against Cargo.lock at commit " + SOURCE.commit + " (SHA-256 " + OLD + ")",
    );
    for (const key of contractDiffKeys(v3, next)) {
      expect(
        CARGO_PRODUCER_LOCK_MOVEMENT.some(
          (allowed) => key === allowed || key.startsWith(allowed + "."),
        ),
      ).toBe(true);
    }
    expect(contractDiffKeys(v3, next)).toContain("toolchain.cargoLockSha256");
    expect(next.platformStatus).toContain("reviewed additive Cargo.lock transition");
    expect(next.platformStatus).toContain("ctor@0.2.9, zfb-css-node@0.0.0");
    expect(next.platformStatus).toContain("only the Cargo.lock hash moved");
    expect(next.baselineEvidence.note).toContain("Reviewed additive Cargo.lock transition");
    const section = readmeSection(next, gzipEvidence, previousVersion, "cargo-producer-lock");
    expect(section).toContain(
      "## v" + v3.toolchain.packageVersion + " cargo-producer-lock transition",
    );
    expect(section).toContain("[workflow run 1]");
    expect(section).toContain("`" + OLD + "` to `" + NEW + "`");
    expect(section).toContain("`ctor@0.2.9`, `zfb-css-node@0.0.0`");
    expect(section).toContain("only `toolchain.cargoLockSha256` moved");
    expect(section).toContain(
      "proved against `Cargo.lock` at commit `" + SOURCE.commit + "` (SHA-256 `" + OLD + "`).",
    );
    expect(rewriteTestPins(testSource, next)).toBe(rewriteTestPins(testSource, v3));
  });

  it("accepts only the reviewed non-additive lock with unchanged artifact identity and totals", () => {
    const lock = reviewedCargoLock();
    const base = structuredClone(v3);
    base.toolchain.cargoLockSha256 = lock.expectedOldSha;
    const measured = {
      ...cargoEvidence,
      cargoLock: lock,
      identity: { ...cargoEvidence.identity, cargoLockSha256: lock.expectedNewSha },
    };
    const next = rebaselineContract(
      base,
      totalsFrom(base),
      measured,
      "cargo-producer-lock",
    ).contract;
    expect(next.toolchain.cargoLockSha256).toBe(lock.expectedNewSha);
    expect(next.ceilings).toEqual(base.ceilings);
    expect(next.allowance).toEqual(base.allowance);
    expect(next.platformStatus).toContain("reviewed non-additive");
    expect(next.baselineEvidence.cargoTransition.changed).toHaveLength(46);
    const raw = totalsFrom(base, { "packed/model": { raw: base.ceilings.packed.model.raw + 1 } });
    expect(() => rebaselineContract(base, raw, measured, "cargo-producer-lock")).toThrow(
      /raw totals changed/,
    );
    expect(() =>
      rebaselineContract(
        base,
        totalsFrom(base),
        { ...measured, identity: { ...measured.identity, cargoLockSha256: OLD } },
        "cargo-producer-lock",
      ),
    ).toThrow(/artifact measured/);
    expect(() =>
      rebaselineContract(
        base,
        totalsFrom(base),
        { ...measured, version: "unreviewed" },
        "cargo-producer-lock",
      ),
    ).toThrow(/packageVersion/);
  });

  it("parses Cargo.lock losslessly and diffs it record by record", () => {
    const parsed = parseCargoLock(newText, "new");
    expect(parsed.header).toBe(
      "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n",
    );
    expect(parsed.packages.map(({ id }) => id)).toEqual([
      "anyhow@1.0.100",
      "ctor@0.2.9",
      "zfb-css@0.0.0",
      "zfb-css-node@0.0.0",
      "zudo-wind@0.0.0",
    ]);
    expect(diffCargoLocks(oldText, newText)).toEqual({
      problems: [],
      added: ADDED,
      changed: [],
      removed: [],
      duplicates: [],
    });
    expect(() =>
      parseCargoLock(
        newText.replace(
          '\n\n[[package]]\nname = "zudo-wind"',
          '\n\n\n[[package]]\nname = "zudo-wind"',
        ),
        "x",
      ),
    ).toThrow(/unexpected record separator/);
    expect(() => parseCargoLock(newText.replace('name = "ctor"\n', ""), "x")).toThrow(
      /without name and version/,
    );
    expect(() => parseCargoLock(newText.slice(0, -1), "x")).toThrow(/does not end with a newline/);
  });

  it("refuses moved totals and a fixture-set difference", () => {
    const raw = totalsFrom(v3, { "packed/model": { raw: v3.ceilings.packed.model.raw + 1 } });
    expect(() => transition(undefined, v3, raw)).toThrow(/raw totals changed \(packed\/model\)/);
    const normalized = totalsFrom(v3, {
      "workspace/model": { gzipNormalized: v3.ceilings.workspace.model.gzipNormalized - 1 },
    });
    expect(() => transition(undefined, v3, normalized)).toThrow(/gzipNormalized totals changed/);
    const missing = totalsFrom(v3);
    delete missing.workspace["json-api"];
    expect(() => transition(undefined, v3, missing)).toThrow(/fixture set differs/);
  });

  it("refuses a moved dependency fingerprint", () => {
    expect(() =>
      transition((e) => ({
        ...e,
        artifactFingerprint: { ...e.artifactFingerprint, sha256: "8".repeat(64) },
        dependencyFingerprint: { ...e.dependencyFingerprint, sha256: "8".repeat(64) },
      })),
    ).toThrow(/cannot move the dependency fingerprint \(sha256 differ/);
    expect(() =>
      transition((e) => ({
        ...e,
        artifactFingerprint: { ...e.artifactFingerprint, sha256: "8".repeat(64) },
      })),
    ).toThrow(/artifact dependency fingerprint sha256 differs from this checkout's/);
  });

  it.each([
    ["packageVersion", (e) => ({ ...e, version: "9.9.9" })],
    ["normalizerSha256", (e) => ({ ...e, normalizerSha256: "e".repeat(64) })],
    ["pnpmLockSha256", (e) => ({ ...e, pnpmLockSha256: "f".repeat(64) })],
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
      "toolchain.nodeVersion",
      (e) => ({
        ...e,
        identity: { ...e.identity, toolchain: { ...e.identity.toolchain, nodeVersion: "v0" } },
      }),
    ],
  ])("refuses a %s movement", (name, edit) => {
    expect(() => transition(edit)).toThrow(
      new RegExp("allows only Cargo.lock to move, but " + name.replace(".", "\\.") + " differ"),
    );
  });

  it("refuses a wrong old or new SHA and an old lock that is not the pinned one", () => {
    const cargo = (patch) => (e) => ({ ...e, cargoLock: { ...e.cargoLock, ...patch } });
    expect(() => transition(cargo({ expectedOldSha: "1".repeat(64) }))).toThrow(
      /the contract pins Cargo.lock [0-9a-f]{64}, not --expected-old-cargo-sha/,
    );
    expect(() => transition(cargo({ oldText: oldText + "\n" }))).toThrow(
      /a{40}:Cargo\.lock hashes to [0-9a-f]{64}, not --expected-old-cargo-sha/,
    );
    // The raw bytes are hashed before decoding: a Buffer proves like its text.
    expect(transition(cargo({ oldText: Buffer.from(oldText) })).contract.toolchain).toEqual({
      ...v3.toolchain,
      cargoLockSha256: NEW,
    });
    for (const source of ["old.lock", { ...SOURCE, commit: "abc" }, { ...SOURCE, path: "" }]) {
      expect(() => transition(cargo({ oldLockSource: source }))).toThrow(
        /the old Cargo\.lock source must be its verified git blob \{commit, path, sha256\}/,
      );
    }
    expect(() =>
      transition(cargo({ oldLockSource: { ...SOURCE, sha256: "1".repeat(64) } })),
    ).toThrow(/must be its verified git blob/);
    expect(() => transition(cargo({ expectedNewSha: "2".repeat(64) }))).toThrow(
      /the checkout's Cargo.lock hashes to [0-9a-f]{64}, not --expected-new-cargo-sha/,
    );
    expect(() =>
      transition((e) => ({ ...e, identity: { ...e.identity, cargoLockSha256: "3".repeat(64) } })),
    ).toThrow(/the artifact measured Cargo.lock 3{64}, not --expected-new-cargo-sha/);
  });

  it("refuses a changed or removed record", () => {
    const changed = newText.replace(
      'version = "1.0.100"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "a23e',
      'version = "1.0.100"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "b23e',
    );
    expect(() => transition(withNew(changed))).toThrow(/changed records anyhow@1.0.100/);
    const removed = newText.replace('[[package]]\nname = "zudo-wind"\nversion = "0.0.0"\n', "");
    expect(() => transition(withNew(removed.replace(/\n\n$/, "\n")))).toThrow(
      /removed records zudo-wind@0.0.0/,
    );
  });

  it("refuses an unexpected or missing addition and a duplicate identity", () => {
    expect(() => transition(withNew(newText, ["ctor@0.2.9"]))).toThrow(
      /unexpected additions zfb-css-node@0.0.0/,
    );
    expect(() => transition(withNew(newText, [...ADDED, "napi@2.16.17"]))).toThrow(
      /allowlisted but not added napi@2.16.17/,
    );
    const duplicate = newText.replace(
      '[[package]]\nname = "ctor"',
      '[[package]]\nname = "anyhow"\nversion = "1.0.100"\nsource = "git+https://example.invalid/anyhow#0"\n\n[[package]]\nname = "ctor"',
    );
    expect(() => transition(withNew(duplicate, [...ADDED, "anyhow@1.0.100"]))).toThrow(
      /duplicate package identities anyhow@1.0.100/,
    );
    expect(() => transition(withNew(newText.replace("version = 4", "version = 3")))).toThrow(
      /the header differs/,
    );
  });

  it("refuses a schemaVersion 2 contract and a lock that did not move", () => {
    expect(() => transition(undefined, asSchemaV2(v3))).toThrow(
      /--reason cargo-producer-lock needs a schemaVersion 3 contract, found schemaVersion 2/,
    );
    const pinnedNew = structuredClone(v3);
    pinnedNew.toolchain.cargoLockSha256 = NEW;
    expect(() =>
      transition(
        (e) => ({ ...e, cargoLock: { ...e.cargoLock, expectedOldSha: NEW, oldText: newText } }),
        pinnedNew,
      ),
    ).toThrow(/--expected-old-cargo-sha equals --expected-new-cargo-sha; no Cargo.lock move/);
  });

  for (const reason of ["release-bump", "runtime-identity"]) {
    it(reason + " refuses any Cargo.lock movement and names cargo-producer-lock", () => {
      expect(() => rebaselineContract(v3, totalsFrom(v3), cargoEvidence, reason)).toThrow(
        new RegExp(
          "--reason " +
            reason +
            " cannot move Cargo.lock \\(the artifact and checkout carry " +
            NEW +
            ", the contract pins " +
            OLD +
            "\\); a reviewed additive producer-lock change needs --reason cargo-producer-lock",
        ),
      );
      const { identity: _identity, ...withoutIdentity } = evidence;
      expect(() =>
        rebaselineContract(contract, totalsFrom(contract), withoutIdentity, reason),
      ).toThrow(/needs the artifact's measured identity inputs/);
    });
  }

  it("requires the four Cargo arguments, takes --old-cargo-lock optionally, for cargo-producer-lock alone", () => {
    const base = [
      "--artifact",
      "dir",
      "--run-id",
      "1",
      "--job-id",
      "2",
      "--head-sha",
      "c".repeat(40),
    ];
    const cargoArgs = {
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-ref": "main",
      "added-packages": ADDED.join(","),
    };
    const argv = (reason, extra) => [
      ...base,
      "--reason",
      reason,
      ...Object.entries(extra).flatMap(([k, v]) => ["--" + k, v]),
    ];
    expect(parseArgs(argv("cargo-producer-lock", cargoArgs))).toMatchObject(cargoArgs);
    const corroborated = { ...cargoArgs, "old-cargo-lock": "old.lock" };
    expect(parseArgs(argv("cargo-producer-lock", corroborated))).toMatchObject(corroborated);
    expect(() => parseArgs(argv("release-bump", { "old-cargo-lock": "old.lock" }))).toThrow(
      "--old-cargo-lock is only for --reason cargo-producer-lock",
    );
    expect(() =>
      parseArgs(argv("cargo-producer-lock", { ...cargoArgs, "old-cargo-ref": "" })),
    ).toThrow("required --old-cargo-ref");
    for (const [ref, message] of [
      ["-x", /--old-cargo-ref must name a git ref, not "-x"/],
      ["main:", /--old-cargo-ref has an empty path/],
      ["main:/etc/Cargo.lock", /--old-cargo-ref path must be relative to the repository root/],
      ["main:../Cargo.lock", /--old-cargo-ref path must not contain a \.\. segment/],
      ["main:a/../Cargo.lock", /must not contain a \.\. segment/],
      ["main:./Cargo.lock", /must be a normalized repository path/],
    ]) {
      expect(() =>
        parseArgs(argv("cargo-producer-lock", { ...cargoArgs, "old-cargo-ref": ref })),
      ).toThrow(message);
    }
    expect(parseOldCargoRef("v1.2.3")).toEqual({ ref: "v1.2.3", path: "Cargo.lock" });
    expect(parseOldCargoRef("abc:crates/x/Cargo.lock")).toEqual({
      ref: "abc",
      path: "crates/x/Cargo.lock",
    });
    for (const key of Object.keys(cargoArgs)) {
      const { [key]: _drop, ...rest } = cargoArgs;
      expect(() => parseArgs(argv("cargo-producer-lock", rest))).toThrow("required --" + key);
      expect(() => parseArgs(argv("release-bump", { [key]: cargoArgs[key] }))).toThrow(
        "--" + key + " is only for --reason cargo-producer-lock",
      );
    }
    expect(() =>
      parseArgs(argv("cargo-producer-lock", { ...cargoArgs, "expected-new-cargo-sha": "abc" })),
    ).toThrow(/--expected-new-cargo-sha must be a 64-character SHA-256/);
    expect(() =>
      parseArgs(argv("cargo-producer-lock", { ...cargoArgs, "added-packages": "ctor" })),
    ).toThrow(/comma-separated list of name@version/);
    expect(parseAddedPackages("a@1, b@2")).toEqual(["a@1", "b@2"]);
    expect(() => parseAddedPackages("a@1,b@2,a@1")).toThrow(/--added-packages repeats a@1/);
    expect(() =>
      parseArgs([
        "--reason",
        "fingerprint-helper",
        "--base-ref",
        "HEAD",
        "--old-cargo-ref",
        "main",
      ]),
    ).toThrow(/takes only --base-ref; remove --old-cargo-ref/);
  });

  it("takes --previous-contract-ref only for the reasons that record repinnedFrom", () => {
    const base = [
      "--artifact",
      "dir",
      "--run-id",
      "1",
      "--job-id",
      "2",
      "--head-sha",
      "c".repeat(40),
    ];
    for (const reason of ["release-bump", "runtime-identity", "lockfile-provenance"]) {
      expect(
        parseArgs([...base, "--reason", reason, "--previous-contract-ref", "main"]),
      ).toMatchObject({ "previous-contract-ref": "main" });
    }
    for (const reason of ["metric-migration", "fingerprint-migration"]) {
      expect(() =>
        parseArgs([...base, "--reason", reason, "--previous-contract-ref", "main"]),
      ).toThrow(
        "--previous-contract-ref is only for --reason release-bump, runtime-identity, lockfile-provenance, cargo-producer-lock, dependency-graph, fingerprint-policy, cargo-internal-rewire",
      );
    }
    expect(() =>
      parseArgs([...base, "--reason", "release-bump", "--previous-contract-ref", "-q"]),
    ).toThrow(/--previous-contract-ref must name a git ref/);
    expect(() =>
      parseArgs([
        "--reason",
        "fingerprint-helper",
        "--base-ref",
        "HEAD",
        "--previous-contract-ref",
        "main",
      ]),
    ).toThrow(/takes only --base-ref; remove --previous-contract-ref/);
  });

  it("records a flat predecessor and refuses a re-pin without a contract reference", () => {
    const repinned = structuredClone(v3);
    repinned.baselineEvidence.repinnedFrom = { outcome: "nested", contract: { sha256: "0" } };
    expect(predecessorRecord(repinned)).toEqual({
      schemaVersion: 3,
      outcome: v3.outcome,
      packageVersion: v3.toolchain.packageVersion,
      baselineSourceSha: v3.baselineSourceSha,
      baselineRunId: v3.baselineEvidence.runId,
    });
    const { previousContract: _p, ...withoutReference } = cargoEvidence;
    for (const reference of [
      undefined,
      { sha256: "abc" },
      { sha256: "d".repeat(64), commit: "a".repeat(40) },
      { sha256: "d".repeat(64), path: "x.json" },
      { sha256: "d".repeat(64), commit: "a".repeat(40), path: "x.json", extra: 1 },
    ]) {
      expect(() =>
        rebaselineContract(
          v3,
          totalsFrom(v3),
          { ...withoutReference, previousContract: reference },
          "cargo-producer-lock",
        ),
      ).toThrow(/a re-pin needs the previous contract reference \{sha256\[, commit, path\]\}/);
    }
    const cited = { path: "p.json", commit: "a".repeat(40), sha256: "d".repeat(64) };
    expect(
      Object.keys(
        transition((e) => ({ ...e, previousContract: cited })).contract.baselineEvidence
          .repinnedFrom.contract,
      ),
    ).toEqual(["sha256", "commit", "path"]);
  });
});

// End-to-end CLI runs against one git-backed test-fixture checkout, built once
// and restored after every case: its only commit holds the OLD Cargo.lock, the
// artifact and the contract pinning the old lock; the working tree carries the
// NEW lock, as a fresh producer-lock move leaves it.
describe("git-backed provenance (#4219)", () => {
  const roots = [];
  const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
  const lockDir = join(root, "scripts/__tests__/fixtures/cargo-producer-lock");
  const oldText = readFileSync(join(lockDir, "old.lock"), "utf8");
  const newText = readFileSync(join(lockDir, "new.lock"), "utf8");
  const OLD = sha(oldText);
  const NEW = sha(newText);
  const ADDED = ["ctor@0.2.9", "zfb-css-node@0.0.0"];
  const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const OUTPUTS = [CONTRACT, TEST, README];
  let repo;
  let artifact;
  let pinned;
  let ambiguous;
  let saved;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const written = () => JSON.parse(read(CONTRACT).toString("utf8"));
  const contractText = (value) => JSON.stringify(value, null, 2) + "\n";

  beforeAll(() => {
    repo = makeGitCheckout(roots, {
      oldCargoLock: oldText,
      newCargoLock: newText,
      prepare: (checkout) => {
        artifact = join(checkout.root, "artifact");
        writeRuns(artifact, Object.keys(liveContract.ceilings.workspace), (mode) => ({
          sourceSha: "b".repeat(40),
          mode,
          platform: { os: "linux", arch: "x64" },
          fixtureVersion: liveContract.fixtureVersion,
          fixtureSha256: checkout.fixtureSha256,
          runnerSha256: checkout.runnerSha256,
          packageVersion: checkout.version,
          runtimePackageVersion: checkout.version,
          honoPackageVersion: liveContract.toolchain.honoVersion,
          esbuildVersion: liveContract.toolchain.esbuildVersion,
          nodeVersion: liveContract.toolchain.nodeVersion,
          zlibVersion: liveContract.toolchain.zlibVersion,
          compression: liveContract.compression,
          repeats: 2,
          pnpmLockSha256: checkout.pnpmLockSha256,
          dependencyFingerprint: {
            policyVersion: checkout.dependencyFingerprint.policyVersion,
            sha256: checkout.dependencyFingerprint.sha256,
          },
          cargoLockSha256: checkout.cargoLockSha256,
        }));
        pinned = structuredClone(liveContract);
        pinned.ceilings = ceilingsOf(artifact);
        pinned.fixtureSha256 = { ...checkout.fixtureSha256 };
        pinned.runnerSha256 = { ...checkout.runnerSha256 };
        pinned.toolchain.packageVersion = checkout.version;
        pinned.toolchain.pnpmLockSha256 = checkout.pnpmLockSha256;
        pinned.toolchain.cargoLockSha256 = OLD;
        pinned.dependencyFingerprint = { ...checkout.dependencyFingerprint };
        writeFileSync(join(checkout.root, CONTRACT), contractText(pinned));
      },
    });
    repo.git("tag", "old-lock");
    repo.git("branch", "dup");
    repo.git("tag", "dup");
    ambiguous = addAmbiguousCommitPrefix(repo.root);
    saved = Object.fromEntries([...OUTPUTS, "Cargo.lock"].map((file) => [file, read(file)]));
  });
  afterEach(() => {
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    rmSync(at("old.lock"), { force: true });
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const argv = (values) =>
    Object.entries(values)
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const cargoArgv = (extra = {}) =>
    argv({
      artifact,
      "run-id": "51",
      "job-id": "52",
      "head-sha": "c".repeat(40),
      reason: "cargo-producer-lock",
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-ref": "HEAD",
      "added-packages": ADDED.join(","),
      ...extra,
    });
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    return vi.spyOn(console, "error").mockImplementation(() => {});
  };

  it("records the old lock's git blob and the HEAD citation of the previous contract", async () => {
    const notices = quiet();
    // Unrelated dirty files (the moved Cargo.lock, an untracked note) do not matter.
    writeFileSync(at("notes.txt"), "test fixture: unrelated untracked file\n");
    try {
      await main(cargoArgv(), repo.root);
    } finally {
      rmSync(at("notes.txt"), { force: true });
    }
    const next = written();
    expect(next.toolchain).toEqual({ ...pinned.toolchain, cargoLockSha256: NEW });
    expect(next.baselineEvidence.cargoTransition).toEqual({
      oldSha: OLD,
      newSha: NEW,
      added: ADDED,
      changed: [],
      removed: [],
      oldLockSource: { commit: repo.commit, path: "Cargo.lock", sha256: OLD },
    });
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 3,
      outcome: pinned.outcome,
      packageVersion: pinned.toolchain.packageVersion,
      baselineSourceSha: pinned.baselineSourceSha,
      baselineRunId: pinned.baselineEvidence.runId,
      contract: { sha256: sha(saved[CONTRACT]), commit: repo.commit, path: CONTRACT },
    });
    expect(notices).not.toHaveBeenCalled();
    const readme = read(README).toString("utf8");
    expect(readme).toContain("`" + OLD + "` to `" + NEW + "`");
    expect(readme).toContain(
      "proved against `Cargo.lock` at commit `" + repo.commit + "` (SHA-256 `" + OLD + "`).",
    );
    expect(JSON.stringify(next)).not.toContain("old.lock");
    expect(contractDiffKeys(pinned, next).every((key) => !key.startsWith("toolchain.pnpm"))).toBe(
      true,
    );
  });

  it("accepts an unambiguous tag or branch, an explicit path and a matching corroboration file", async () => {
    quiet();
    writeFileSync(at("old.lock"), oldText);
    for (const ref of ["old-lock", "main:Cargo.lock", repo.commit.slice(0, 12)]) {
      for (const file of OUTPUTS) writeFileSync(at(file), saved[file]);
      await main(cargoArgv({ "old-cargo-ref": ref, "old-cargo-lock": at("old.lock") }), repo.root);
      expect(written().baselineEvidence.cargoTransition.oldLockSource).toEqual({
        commit: repo.commit,
        path: "Cargo.lock",
        sha256: OLD,
      });
    }
  });

  // Each refusal leaves the contract, the budget test and the README byte-identical.
  it.each([
    ["an option-like ref", { "old-cargo-ref": "-x" }, null, /--old-cargo-ref must name a git ref/],
    [
      "an unresolvable ref",
      { "old-cargo-ref": "no-such-ref" },
      null,
      /--old-cargo-ref no-such-ref does not resolve to exactly one commit in /,
    ],
    [
      "an ambiguous short SHA",
      () => ({ "old-cargo-ref": ambiguous }),
      null,
      /does not resolve to exactly one commit .*short object ID [0-9a-f]{4} is ambiguous/,
    ],
    [
      "an ambiguous branch and tag name",
      { "old-cargo-ref": "dup" },
      null,
      /refname 'dup' is ambiguous/,
    ],
    [
      "a tree ref",
      { "old-cargo-ref": "HEAD^{tree}" },
      null,
      /expected commit type, but the object dereferences to tree type/,
    ],
    [
      "a blob ref",
      () => ({ "old-cargo-ref": repo.git("rev-parse", "HEAD:Cargo.lock") }),
      null,
      /expected commit type, but the object dereferences to blob type/,
    ],
    ["an empty path", { "old-cargo-ref": "HEAD:" }, null, /--old-cargo-ref has an empty path/],
    [
      "an absolute path",
      { "old-cargo-ref": "HEAD:/Cargo.lock" },
      null,
      /path must be relative to the repository root/,
    ],
    [
      "a .. path",
      { "old-cargo-ref": "HEAD:../Cargo.lock" },
      null,
      /path must not contain a \.\. segment/,
    ],
    [
      "a missing blob",
      { "old-cargo-ref": "HEAD:no-such.lock" },
      null,
      /--old-cargo-ref [0-9a-f]{40}:no-such\.lock does not exist/,
    ],
    [
      "a path that is a tree",
      { "old-cargo-ref": "HEAD:research" },
      null,
      /[0-9a-f]{40}:research is a tree, not a blob/,
    ],
    [
      "a blob whose hash is not --expected-old-cargo-sha",
      { "old-cargo-ref": "HEAD:pnpm-lock.yaml" },
      null,
      /[0-9a-f]{40}:pnpm-lock\.yaml hashes to [0-9a-f]{64}, not --expected-old-cargo-sha/,
    ],
    [
      "a corroboration file that differs from the blob",
      () => ({ "old-cargo-lock": at("old.lock") }),
      () => writeFileSync(at("old.lock"), newText),
      /--old-cargo-lock differs from [0-9a-f]{40}:Cargo\.lock/,
    ],
    [
      "an explicit --previous-contract-ref that is not the input bytes",
      { "previous-contract-ref": "HEAD" },
      () => writeFileSync(at(CONTRACT), JSON.stringify(pinned, null, 4) + "\n"),
      /--previous-contract-ref HEAD: [0-9a-f]{40}:research\/v3-island-size\/decision-linux-x64\.json is not byte-identical to the input contract; refusing/,
    ],
    [
      "an unresolvable --previous-contract-ref (no HEAD fallback)",
      { "previous-contract-ref": "no-such-ref" },
      null,
      /--previous-contract-ref no-such-ref does not resolve to exactly one commit/,
    ],
    [
      "a missing test-pin anchor",
      {},
      () =>
        writeFileSync(
          at(TEST),
          saved[TEST].toString("utf8").replace("records the reviewed Linux x64 ceilings", "x"),
        ),
      /Linux ceiling test not found/,
    ],
    [
      "missing ceiling pins inside the anchored test",
      {},
      () =>
        writeFileSync(
          at(TEST),
          saved[TEST]
            .toString("utf8")
            .replace("linuxContract.ceilings.packed).toEqual({", "linuxContract.x).toEqual({"),
        ),
      /missing packed pins in /,
    ],
  ])("refuses %s and writes nothing", async (_name, extra, setup, message) => {
    quiet();
    setup?.();
    const before = OUTPUTS.map(read);
    await expect(
      main(cargoArgv(typeof extra === "function" ? extra() : extra), repo.root),
    ).rejects.toThrow(message);
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));
  });

  it("resolves refs in the supplied linked worktree, at its detached HEAD", async () => {
    quiet();
    const dir = mkdtempSync(join(tmpdir(), "zfb-island-linked-"));
    roots.push(dir);
    const tree = join(dir, "wt");
    repo.git("worktree", "add", "-q", "--detach", tree, "HEAD");
    try {
      const linked = gitIn(tree);
      writeFileSync(join(tree, "notes.txt"), "test fixture: linked-worktree commit\n");
      linked("add", "notes.txt");
      linked("commit", "-qm", "test fixture: linked worktree");
      const head = linked("rev-parse", "HEAD");
      expect(head).not.toBe(repo.commit);
      expect(head).not.toBe(gitIn(root)("rev-parse", "HEAD"));
      writeFileSync(join(tree, "Cargo.lock"), newText);
      await main(cargoArgv(), tree);
      const next = JSON.parse(readFileSync(join(tree, CONTRACT), "utf8"));
      expect(next.baselineEvidence.cargoTransition.oldLockSource.commit).toBe(head);
      expect(next.baselineEvidence.repinnedFrom.contract).toEqual({
        sha256: sha(saved[CONTRACT]),
        commit: head,
        path: CONTRACT,
      });
      expect(written()).toEqual(pinned);
    } finally {
      repo.git("worktree", "remove", "--force", tree);
    }
  });

  it("records only the digest, with a notice, for a reformatted contract", async () => {
    const notices = quiet();
    const reformatted = JSON.stringify(pinned, null, 4) + "\n";
    writeFileSync(at(CONTRACT), reformatted);
    await main(cargoArgv(), repo.root);
    const reference = written().baselineEvidence.repinnedFrom.contract;
    expect(reference).toEqual({ sha256: sha(reformatted) });
    expect(Object.keys(reference)).toEqual(["sha256"]);
    expect(notices.mock.calls.flat().join("\n")).toContain(
      "previous contract bytes are not at HEAD; recorded digest only — no retrievable citation",
    );
  });

  it("re-pins twice: each repinnedFrom names only its immediate predecessor", async () => {
    const notices = quiet();
    // The previous release's contract: the checkout's Cargo.lock, an older version.
    const previous = structuredClone(pinned);
    previous.toolchain.cargoLockSha256 = NEW;
    previous.toolchain.packageVersion = "0.0.1";
    writeFileSync(at(CONTRACT), contractText(previous));
    const run = (reason, runId, headSha) =>
      main(
        argv({ artifact, "run-id": runId, "job-id": "9", "head-sha": headSha, reason }),
        repo.root,
      );
    await run("release-bump", "61", "e".repeat(40));
    const first = written();
    const firstBytes = read(CONTRACT);
    const version = repo.version;
    expect(first.outcome).toBe("release-version-stamp");
    expect(first.toolchain.packageVersion).toBe(version);
    expect(first.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 3,
      outcome: pinned.outcome,
      packageVersion: "0.0.1",
      baselineSourceSha: pinned.baselineSourceSha,
      baselineRunId: pinned.baselineEvidence.runId,
      contract: { sha256: sha(contractText(previous)) },
    });
    // Release-version wording for a changed package version.
    expect(first.platformStatus).toContain("This records a release-bump re-pin");
    expect(first.platformStatus).toContain("unchanged from the v0.0.1 contract");
    expect(
      first.baselineEvidence.note.startsWith("Release probe run for v" + version + " at "),
    ).toBe(true);

    await run("runtime-identity", "62", "f".repeat(40));
    const second = written();
    expect(Object.keys(second.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(second.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 3,
      outcome: "release-version-stamp",
      packageVersion: version,
      baselineSourceSha: "e".repeat(40),
      baselineRunId: 61,
      contract: { sha256: sha(firstBytes) },
    });
    expect(JSON.stringify(second.baselineEvidence.repinnedFrom)).not.toContain("repinnedFrom");
    // Same-version wording that never claims a release.
    expect(second.outcome).toBe("runtime-identity-repin");
    expect(second.platformStatus).toContain("This records a runtime-identity re-pin");
    expect(second.platformStatus).not.toMatch(/release/i);
    expect(
      second.baselineEvidence.note.startsWith(
        "Runtime-identity re-pin run at v" + version + " at ",
      ),
    ).toBe(true);
    expect(second.baselineEvidence.note).not.toMatch(/release/i);

    const sections = read(README).toString("utf8").split("\n## ").slice(1);
    expect(sections).toHaveLength(2);
    expect(sections[0]).toContain("v" + version + " release baseline");
    expect(sections[0]).toContain("re-pinned for the v" + version + " release");
    expect(sections[0]).toContain("unchanged from the v0.0.1 contract");
    expect(sections[1]).toContain("v" + version + " runtime-identity re-pin");
    expect(sections[1]).not.toMatch(/release/i);
    // Neither contract was at HEAD: digest only, each time with a notice.
    expect(notices).toHaveBeenCalledTimes(2);
  });
});

describe("assertAllowedMovement (#4218)", () => {
  const before = {
    outcome: "a",
    dependencyFingerprint: { sha256: "1", helperSha256: "2" },
    baselineEvidence: { runId: 1 },
    nested: { a: 1, b: 2 },
  };
  it("matches exact keys, a trailing-dot prefix and baselineEvidence as a whole", () => {
    const after = structuredClone(before);
    after.outcome = "b";
    after.dependencyFingerprint.sha256 = "3";
    after.baselineEvidence = { runId: 2, repinnedFrom: { x: 1 } };
    after.nested.a = 9;
    expect(() =>
      assertAllowedMovement(
        "dependency-graph",
        ["outcome", "dependencyFingerprint.sha256", "baselineEvidence", "nested."],
        before,
        after,
      ),
    ).not.toThrow();
    // `dependencyFingerprint` alone is an exact key, not a prefix.
    after.dependencyFingerprint.helperSha256 = "4";
    expect(() =>
      assertAllowedMovement(
        "dependency-graph",
        ["outcome", "dependencyFingerprint", "baselineEvidence", "nested."],
        before,
        after,
      ),
    ).toThrow(
      "--reason dependency-graph: would move dependencyFingerprint.helperSha256, dependencyFingerprint.sha256; refusing",
    );
  });
});

// End-to-end CLI runs against one test-fixture checkout — synthetic prior
// graph, not native evidence: its pnpm-lock.yaml has one reachable snapshot
// moved, the contract pins the real lock's (prior) fingerprint, and the
// artifact was measured at the checkout carrying the new graph.
describe("dependency-graph (#4218)", () => {
  const roots = [];
  const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const INVENTORY = "research/v3-island-size/lock-fingerprint-inventory.json";
  const OUTPUTS = [CONTRACT, TEST, README, INVENTORY];
  const SOURCE = "b".repeat(40);
  const HEAD = "c".repeat(40);
  let repo;
  let artifact;
  let pinned;
  let saved;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const written = () => JSON.parse(read(CONTRACT).toString("utf8"));
  const contractText = (value) => JSON.stringify(value, null, 2) + "\n";
  const tempDir = (prefix) => {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    roots.push(dir);
    return dir;
  };

  // A Linux artifact measured at the checkout; `edit` adjusts each mode's
  // provenance and `tamper` the saved files afterwards.
  function linuxArtifact(edit, tamper) {
    const dir = tempDir("zfb-graph-artifact-");
    writeRuns(dir, Object.keys(liveContract.ceilings.workspace), (mode) => {
      const provenance = {
        sourceSha: SOURCE,
        mode,
        platform: { os: "linux", arch: "x64" },
        fixtureVersion: liveContract.fixtureVersion,
        fixtureSha256: repo.fixtureSha256,
        runnerSha256: repo.runnerSha256,
        packageVersion: repo.version,
        runtimePackageVersion: repo.version,
        honoPackageVersion: liveContract.toolchain.honoVersion,
        esbuildVersion: liveContract.toolchain.esbuildVersion,
        nodeVersion: liveContract.toolchain.nodeVersion,
        zlibVersion: liveContract.toolchain.zlibVersion,
        compression: liveContract.compression,
        repeats: 2,
        pnpmLockSha256: repo.pnpmLockSha256,
        dependencyFingerprint: {
          policyVersion: repo.dependencyFingerprint.policyVersion,
          sha256: repo.dependencyFingerprint.sha256,
        },
        cargoLockSha256: repo.cargoLockSha256,
      };
      edit?.(provenance, mode);
      return provenance;
    });
    tamper?.(dir);
    return dir;
  }

  beforeAll(() => {
    const version = JSON.parse(readFileSync(join(root, "packages/zfb/package.json"), "utf8"));
    const moved = moveReachableSnapshot(
      readFileSync(join(root, "pnpm-lock.yaml"), "utf8"),
      version.version,
    );
    repo = makeGraphCheckout(roots, { pnpmLock: moved.lockText });
    artifact = linuxArtifact();
    pinned = structuredClone(liveContract);
    pinned.ceilings = ceilingsOf(artifact);
    pinned.fixtureSha256 = { ...repo.fixtureSha256 };
    pinned.runnerSha256 = { ...repo.runnerSha256 };
    pinned.toolchain.packageVersion = repo.version;
    pinned.toolchain.pnpmLockSha256 = repo.priorPnpmLockSha256;
    pinned.toolchain.cargoLockSha256 = repo.cargoLockSha256;
    pinned.dependencyFingerprint = { ...repo.priorFingerprint };
    writeFileSync(at(CONTRACT), contractText(pinned));
    saved = Object.fromEntries(OUTPUTS.map((file) => [file, read(file)]));
  });
  afterEach(() => {
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const argv = (extra = {}) =>
    Object.entries({
      artifact,
      "run-id": "71",
      "job-id": "72",
      "head-sha": HEAD,
      "source-sha": SOURCE,
      reason: "dependency-graph",
      stage: join(tempDir("zfb-graph-stage-"), "stage"),
      ...extra,
    })
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    return vi.spyOn(console, "error").mockImplementation(() => {});
  };
  const unchanged = (before) =>
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));

  it("re-pins only the fingerprint sha256 end to end and regenerates the inventory", async () => {
    quiet();
    expect(repo.dependencyFingerprint.sha256).not.toBe(repo.priorFingerprint.sha256);
    const stage = join(tempDir("zfb-graph-stage-"), "stage");
    await main(argv({ stage }), repo.root);
    unchanged(OUTPUTS.map((file) => saved[file]));
    landStaged(stage, repo.root);
    const next = written();
    const moved = contractDiffKeys(pinned, next);
    expect(moved.filter((key) => !key.startsWith("baselineEvidence."))).toEqual([
      "baselineSourceSha",
      "dependencyFingerprint.sha256",
      "outcome",
      "platformStatus",
      "toolchain.pnpmLockSha256",
    ]);
    expect(moved.some((key) => key.startsWith("baselineEvidence."))).toBe(true);
    expect(() =>
      assertAllowedMovement("dependency-graph", DEPENDENCY_GRAPH_MOVEMENT, pinned, next),
    ).not.toThrow();
    expect(next.dependencyFingerprint).toEqual({
      ...pinned.dependencyFingerprint,
      sha256: repo.dependencyFingerprint.sha256,
    });
    expect(next.toolchain).toEqual({ ...pinned.toolchain, pnpmLockSha256: repo.pnpmLockSha256 });
    expect(next.ceilings).toEqual(pinned.ceilings);
    expect(next.outcome).toBe("dependency-graph-repin");
    expect(next.baselineSourceSha).toBe(HEAD);
    expect(next.baselineEvidence).toMatchObject({ event: "ci", runId: 71, jobId: 72 });
    expect(next.baselineEvidence.sourceSha).toBe(SOURCE);
    expect(next.baselineEvidence.fingerprintTransition).toEqual({
      policyVersion: 1,
      oldSha256: repo.priorFingerprint.sha256,
      newSha256: repo.dependencyFingerprint.sha256,
    });
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.baselineEvidence.note.startsWith("Dependency-graph re-pin run at v")).toBe(true);
    expect(next.platformStatus).toContain(
      "equal totals alone do not prove byte-identical product code",
    );
    expect(next.platformStatus).toContain(
      "PR merge source " + SOURCE + " (probe head " + HEAD + ")",
    );
    // The inventory is the checkout's own --explain output, byte for byte.
    expect(read(INVENTORY).equals(helperExplainBytes(repo.root))).toBe(true);
    expect(read(INVENTORY).equals(saved[INVENTORY])).toBe(false);
    const readme = read(README).toString("utf8");
    expect(readme.startsWith(saved[README].toString("utf8"))).toBe(true);
    expect(readme).toContain("## v" + repo.version + " dependency-graph re-pin");
    expect(readme).toContain(
      "`" + repo.priorFingerprint.sha256 + "` to `" + repo.dependencyFingerprint.sha256 + "`",
    );
    expect(readme).toContain("[workflow run 71]");
    expect(read(TEST).toString("utf8")).toBe(rewriteTestPins(saved[TEST].toString("utf8"), next));
  });

  // Each refusal leaves the contract, the budget test, the README and the inventory unchanged.
  const fingerprintKey = (key, value) => ({
    name: "a moved " + key,
    contract: (c) => (c.dependencyFingerprint[key] = value),
    message: new RegExp(
      "--reason dependency-graph moves only dependencyFingerprint\\.sha256, but " +
        key +
        " differ from the contract; .*`fingerprint-policy` \\(#4218\\)",
    ),
  });
  const identity = (name, edit) => ({
    name: "a changed " + name,
    contract: edit,
    message: new RegExp(
      "--reason dependency-graph moves only dependencyFingerprint\\.sha256, but " +
        name.replace(".", "\\.") +
        " differ from the contract",
    ),
  });
  const sidecar = (mode, edit) => (dir) => {
    const path = join(dir, "real-" + mode, "measurement.normalized.json");
    const value = JSON.parse(readFileSync(path, "utf8"));
    edit(value);
    writeFileSync(path, JSON.stringify(value));
  };
  it.each([
    {
      name: "an unchanged sha256 (no-op)",
      contract: (c) => (c.dependencyFingerprint = { ...repo.dependencyFingerprint }),
      message:
        "--reason dependency-graph: the contract already pins this fingerprint; nothing to re-pin",
    },
    fingerprintKey("policyVersion", 2),
    fingerprintKey("parser", "yaml@2.8.0"),
    fingerprintKey("helperSha256", "1".repeat(64)),
    {
      name: "raw +1",
      contract: (c) => (c.ceilings.workspace.model.raw -= 1),
      message: /raw totals changed \(workspace\/model\); this is not a dependency-graph re-pin/,
    },
    {
      name: "raw -1",
      contract: (c) => (c.ceilings.packed.model.raw += 1),
      message: /raw totals changed \(packed\/model\)/,
    },
    {
      name: "gzipNormalized +1",
      contract: (c) => (c.ceilings.workspace["show-for"].gzipNormalized -= 1),
      message: /gzipNormalized totals changed \(workspace\/show-for \+1\)/,
    },
    {
      name: "gzipNormalized -1",
      contract: (c) => (c.ceilings.packed["json-api"].gzipNormalized += 1),
      message: /gzipNormalized totals changed \(packed\/json-api -1\)/,
    },
    {
      name: "a changed Cargo.lock",
      contract: (c) => (c.toolchain.cargoLockSha256 = "0".repeat(64)),
      message:
        /--reason dependency-graph cannot move Cargo\.lock .*needs --reason cargo-producer-lock first/,
    },
    identity("packageVersion", (c) => (c.toolchain.packageVersion = "0.0.1")),
    ...["honoVersion", "esbuildVersion", "nodeVersion", "zlibVersion"].map((key) =>
      identity("toolchain." + key, (c) => (c.toolchain[key] = "0.0.0-test")),
    ),
    {
      name: "a changed fixture set",
      contract: (c) => (c.ceilings.workspace.extra = { raw: 1, gzipNormalized: 1 }),
      message: /fixture set differs from the contract \(workspace\/extra\)/,
    },
    identity("fixtureSha256", (c) => (c.fixtureSha256.model = "0".repeat(64))),
    identity("runnerSha256", (c) => (c.runnerSha256["measure.mjs"] = "0".repeat(64))),
    identity("normalizerSha256", (c) => (c.normalizerSha256 = "e".repeat(64))),
    identity("allowance", (c) => (c.allowance = { raw: 1, gzipNormalized: 0 })),
    identity("metric", (c) => (c.metric = "gzip")),
    {
      name: "a schemaVersion 2 contract",
      contract: (c) => {
        delete c.dependencyFingerprint;
        c.schemaVersion = 2;
      },
      message: /--reason dependency-graph needs a schemaVersion 3 contract, found schemaVersion 2/,
    },
    {
      name: "a wrong-platform artifact",
      provenance: (p) => (p.platform = { os: "darwin", arch: "arm64" }),
      message: /not linux-x64; Darwin arm64 uses rebaseline-island-size-darwin\.mjs/,
    },
    {
      name: "a workspace/packed fingerprint mismatch",
      provenance: (p, mode) => {
        if (mode === "packed") p.dependencyFingerprint.sha256 = "0".repeat(64);
      },
      message: /packed artifact dependency fingerprint sha256 differs from this checkout's/,
    },
    {
      name: "an artifact fingerprint that is not the checkout's",
      provenance: (p) => (p.dependencyFingerprint.sha256 = "0".repeat(64)),
      message: /workspace artifact dependency fingerprint sha256 differs from this checkout's/,
    },
    {
      name: "a sidecar whose results were edited",
      tamper: sidecar("packed", (s) => (s.results.model.shipped.gzipNormalized -= 1)),
      message:
        /real-packed: measurement\.normalized\.json results disagree with the recomputation from the saved bytes/,
    },
    {
      name: "a sidecar whose source.sha256 is stale",
      tamper: sidecar("workspace", (s) => (s.source.sha256 = "0".repeat(64))),
      message: /real-workspace: measurement\.normalized\.json source\.sha256 .*\(stale sidecar\)/,
    },
    {
      name: "a sidecar whose normalizer.sha256 was tampered",
      tamper: sidecar("packed", (s) => (s.normalizer.sha256 = "0".repeat(64))),
      message:
        "real-packed: measurement.normalized.json normalizer.sha256 differs from the current normalizer; refusing",
    },
    {
      name: "a --source-sha that is not the artifact's",
      args: { "source-sha": "d".repeat(40) },
      message: /workspace artifact measured source b{40}, not --source-sha/,
    },
    {
      name: "a cargo-only flag",
      args: { "added-packages": "ctor@0.2.9" },
      message: "--added-packages is only for --reason cargo-producer-lock",
    },
    {
      name: "a missing test-pin anchor",
      setup: () =>
        writeFileSync(
          at(TEST),
          saved[TEST].toString("utf8").replace("records the reviewed Linux x64 ceilings", "x"),
        ),
      message: /Linux ceiling test not found/,
    },
    {
      name: "a missing measurement.json",
      tamper: (dir) => rmSync(join(dir, "real-packed", "measurement.json")),
      message: "artifact is missing real-packed/measurement.json",
    },
    {
      name: "a missing measurement.normalized.json",
      tamper: (dir) => rmSync(join(dir, "real-workspace", "measurement.normalized.json")),
      message: "real-workspace: missing measurement.normalized.json; refusing",
    },
    {
      name: "a missing fixture directory",
      tamper: (dir) => rmSync(join(dir, "real-workspace", "model"), { recursive: true }),
      message: /saved case directories differ from measurement\.json results/,
    },
    {
      name: "an extra fixture directory",
      tamper: (dir) => {
        for (const pass of ["pass-1", "pass-2"]) {
          mkdirSync(join(dir, "real-packed", "extra", pass), { recursive: true });
        }
      },
      message: /saved case directories differ from measurement\.json results/,
    },
  ])(
    "refuses $name and writes nothing",
    async ({ contract: edit, provenance, tamper, args, setup, message }) => {
      quiet();
      if (edit) {
        const contract = structuredClone(pinned);
        edit(contract);
        writeFileSync(at(CONTRACT), contractText(contract));
      }
      setup?.();
      const dir = provenance || tamper ? linuxArtifact(provenance, tamper) : artifact;
      const before = OUTPUTS.map(read);
      await expect(main(argv({ artifact: dir, ...args }), repo.root)).rejects.toThrow(message);
      unchanged(before);
    },
  );

  it.each([
    ["an unknown --reason", { reason: "dependency-graphs" }, /--reason must be one of /],
    ["an empty --reason", { reason: "" }, "required --reason"],
    ["a missing --source-sha", { "source-sha": undefined }, "required --source-sha"],
    ["a malformed --source-sha", { "source-sha": "abc" }, /--source-sha must be a full 40/],
    ["a missing --run-id", { "run-id": undefined }, "required --run-id"],
    ["an empty --stage", { stage: "" }, "--stage needs a directory"],
  ])("refuses %s in parseArgs, before reading anything", async (_name, extra, message) => {
    const missing = join(tmpdir(), "zfb-no-such-artifact-" + process.pid);
    expect(() => parseArgs(argv({ artifact: missing, ...extra }))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(argv({ artifact: missing, ...extra }), repo.root)).rejects.toThrow(message);
    unchanged(before);
    expect(() => parseArgs(["--reason", "dependency-graph", "--artifact"])).toThrow(
      /bad argument: --artifact/,
    );
  });

  it("refuses to run without --stage, in parseArgs, before reading or writing anything", async () => {
    const message =
      "--reason dependency-graph re-pins both platforms together and only stages: pass --stage <dir> for Linux x64 and Darwin arm64 into the same directory, then --apply <dir> (nothing was written)";
    expect(() => parseArgs(argv({ stage: undefined }))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(argv({ stage: undefined }), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });

  it("stages every output outside the checkout and refuses an unpaired --apply", async () => {
    quiet();
    const stage = join(tempDir("zfb-graph-stage-"), "stage");
    await main(argv({ stage }), repo.root);
    unchanged(OUTPUTS.map((file) => saved[file]));
    const manifest = JSON.parse(readFileSync(join(stage, "stage-manifest.json"), "utf8"));
    const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
    expect(Object.keys(manifest.destinations)).toEqual([...OUTPUTS].sort());
    expect(manifest.runs).toEqual([
      {
        reason: "dependency-graph",
        platform: "linux-x64",
        sourceSha: SOURCE,
        headSha: HEAD,
        runId: 71,
        artifactId: null,
        previousContract: { sha256: sha(saved[CONTRACT]) },
        transition: {
          from: {
            packageVersion: pinned.toolchain.packageVersion,
            cargoLockSha256: pinned.toolchain.cargoLockSha256,
            dependencyFingerprint: pinned.dependencyFingerprint,
          },
          to: {
            packageVersion: pinned.toolchain.packageVersion,
            cargoLockSha256: pinned.toolchain.cargoLockSha256,
            pnpmLockSha256: repo.pnpmLockSha256,
            dependencyFingerprint: {
              ...pinned.dependencyFingerprint,
              sha256: repo.dependencyFingerprint.sha256,
            },
          },
        },
      },
    ]);
    for (const file of OUTPUTS) {
      expect(manifest.destinations[file]).toEqual({
        live: sha(saved[file]),
        staged: sha(readFileSync(join(stage, file))),
      });
    }
    expect(JSON.parse(readFileSync(join(stage, CONTRACT), "utf8")).outcome).toBe(
      "dependency-graph-repin",
    );
    expect(() => applyStage(stage, repo.root)).toThrow(
      /--reason dependency-graph is not staged for darwin-arm64; stage both platforms into the same directory first \(nothing was applied\)/,
    );
    unchanged(OUTPUTS.map((file) => saved[file]));
    // A refused re-pin leaves an empty stage directory empty.
    const empty = tempDir("zfb-graph-stage-empty-");
    const contract = structuredClone(pinned);
    contract.dependencyFingerprint = { ...repo.dependencyFingerprint };
    writeFileSync(at(CONTRACT), contractText(contract));
    await expect(main(argv({ stage: empty }), repo.root)).rejects.toThrow(/nothing to re-pin/);
    expect(readdirSync(empty)).toEqual([]);
  });

  it.each([
    ["the checkout itself", () => repo.root, /resolves to or contains the checkout/],
    [
      "a parent of the checkout",
      () => join(repo.root, ".."),
      /resolves to or contains the checkout/,
    ],
    [
      "a directory holding a live destination",
      () => join(repo.root, "research"),
      /the stage directory contains the live destination research\/v3-island-size\//,
    ],
    [
      "a directory whose research/ links to the checkout's",
      () => {
        const dir = tempDir("zfb-graph-stage-alias-");
        symlinkSync(join(repo.root, "research"), join(dir, "research"), "dir");
        return dir;
      },
      /the staged research\/v3-island-size\/.* aliases the live destination/,
    ],
  ])("refuses a stage directory that is %s", async (_name, dirOf, message) => {
    quiet();
    const before = OUTPUTS.map(read);
    await expect(main(argv({ stage: dirOf() }), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });
});

describe("fingerprintPolicyRefusal (#4218)", () => {
  const pinned = {
    policyVersion: 1,
    parser: "yaml@2.9.0",
    helperSha256: "1".repeat(64),
    sha256: "2".repeat(64),
  };
  const local = {
    policyVersion: 2,
    parser: "yaml@2.9.0",
    helperSha256: "3".repeat(64),
    sha256: "4".repeat(64),
  };
  const helper = { policyVersion: 2, parser: "yaml@2.9.0", helperSha256: "3".repeat(64) };
  const refusal = (contractFingerprint, localFingerprint = local, helperIdentity = helper) =>
    fingerprintPolicyRefusal(
      { dependencyFingerprint: contractFingerprint },
      localFingerprint,
      helperIdentity,
    );

  it("accepts exactly one policyVersion step that the helper exports", () => {
    expect(refusal(pinned)).toBeNull();
    expect(
      refusal(pinned, { ...local, parser: "yaml@3.0.0" }, { ...helper, parser: "yaml@3.0.0" }),
    ).toBeNull();
  });

  it.each([
    [
      "+0 with only sha256 moved",
      { ...local, sha256: "5".repeat(64) },
      local,
      helper,
      "policyVersion did not move; use fingerprint-helper or dependency-graph",
    ],
    [
      "+0 with nothing moved",
      local,
      local,
      helper,
      "policyVersion did not move; use fingerprint-helper or dependency-graph",
    ],
    [
      "+0 with the parser moved",
      { ...local, parser: "yaml@2.8.0" },
      local,
      helper,
      "parser moved from yaml@2.8.0 to yaml@2.9.0 without a policyVersion step",
    ],
    [
      "+0 with only helperSha256 moved",
      { ...local, helperSha256: "6".repeat(64) },
      local,
      helper,
      "helperSha256 moved without a policyVersion step; a helper-only edit needs `fingerprint-helper`",
    ],
    ["+2", { ...pinned, policyVersion: 0 }, local, helper, "must increase by exactly one"],
    ["-1", { ...pinned, policyVersion: 3 }, local, helper, "must increase by exactly one"],
    [
      "a helper exporting another version",
      pinned,
      local,
      { ...helper, policyVersion: 3 },
      "does not match the helper's exported FINGERPRINT_POLICY_VERSION 3",
    ],
    [
      "a parser the helper does not export",
      pinned,
      local,
      { ...helper, parser: "yaml@3.0.0" },
      "does not match the helper's exported FINGERPRINT_PARSER yaml@3.0.0",
    ],
    [
      "a helperSha256 that is not the helper's",
      pinned,
      local,
      { ...helper, helperSha256: "7".repeat(64) },
      "does not match the checkout helper's fingerprintHelperSha256()",
    ],
    [
      "a non-integer policyVersion",
      { ...pinned, policyVersion: "1" },
      local,
      helper,
      "policyVersion must be an integer",
    ],
  ])("refuses %s", (_name, contractFingerprint, localFingerprint, helperIdentity, message) => {
    expect(refusal(contractFingerprint, localFingerprint, helperIdentity)).toContain(message);
  });

  it("rewrites only the policy identity line and refuses a document without it", () => {
    const doc =
      '# Policy\n\nPolicy identity: `policyVersion: 1`, `parser: "yaml@2.9.0"`. Any change.\n';
    const next = rewritePolicyIdentity(doc, { policyVersion: 2, parser: "yaml@2.9.0+fixture" });
    expect(next).toBe(
      '# Policy\n\nPolicy identity: `policyVersion: 2`, `parser: "yaml@2.9.0+fixture"`. Any change.\n',
    );
    expect(rewritePolicyIdentity(next, { policyVersion: 2, parser: "yaml@2.9.0+fixture" })).toBe(
      next,
    );
    expect(() => rewritePolicyIdentity("# Policy\n", local)).toThrow(
      "--reason fingerprint-policy: policy identity line not found in LOCK-FINGERPRINT-POLICY.md",
    );
  });

  it("refuses a document with more than one policy identity line", () => {
    const line = 'Policy identity: `policyVersion: 1`, `parser: "yaml@2.9.0"`.';
    for (const second of [line, "Policy identity: (malformed)"]) {
      expect(() =>
        rewritePolicyIdentity("# Policy\n\n" + line + "\n\n" + second + "\n", local),
      ).toThrow(
        "--reason fingerprint-policy: 2 policy identity lines in LOCK-FINGERPRINT-POLICY.md; expected exactly one",
      );
    }
  });

  it("round-trips a parser that needs JSON escapes or carries a replacement pattern", () => {
    const doc =
      '# Policy\n\nPolicy identity: `policyVersion: 1`, `parser: "yaml@2.9.0"`. Any change.\n';
    const escaped = { policyVersion: 2, parser: 'yaml@2.9.0+"q"\\$&' };
    const next = rewritePolicyIdentity(doc, escaped);
    expect(next).toBe(
      '# Policy\n\nPolicy identity: `policyVersion: 2`, `parser: "yaml@2.9.0+\\"q\\"\\\\$&"`. Any change.\n',
    );
    expect(rewritePolicyIdentity(next, escaped)).toBe(next);
    expect(rewritePolicyIdentity(next, { policyVersion: 3, parser: "yaml@3.0.0" })).toBe(
      '# Policy\n\nPolicy identity: `policyVersion: 3`, `parser: "yaml@3.0.0"`. Any change.\n',
    );
  });
});

// End-to-end CLI runs against one test-fixture checkout — synthetic prior
// policy, not native evidence: the checkout's COPY of lock-fingerprint.mjs
// computes policy 2, the contract pins the live (policy 1) helper's
// fingerprint of the same lock, and the artifact was measured at the
// checkout carrying the policy-2 helper.
describe("fingerprint-policy (#4218)", () => {
  const roots = [];
  const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const INVENTORY = "research/v3-island-size/lock-fingerprint-inventory.json";
  const POLICY = "research/v3-island-size/LOCK-FINGERPRINT-POLICY.md";
  const OUTPUTS = [CONTRACT, TEST, README, INVENTORY, POLICY];
  const SOURCE = "b".repeat(40);
  const HEAD = "c".repeat(40);
  let repo;
  let artifact;
  let pinned;
  let saved;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const written = () => JSON.parse(read(CONTRACT).toString("utf8"));
  const contractText = (value) => JSON.stringify(value, null, 2) + "\n";
  const tempDir = (prefix) => {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    roots.push(dir);
    return dir;
  };

  // A Linux artifact measured at `checkout`; `edit` adjusts each mode's
  // provenance and `tamper` the saved files afterwards.
  function artifactAt(checkout, edit, tamper) {
    const dir = tempDir("zfb-policy-artifact-");
    writeRuns(dir, Object.keys(liveContract.ceilings.workspace), (mode) => {
      const provenance = {
        sourceSha: SOURCE,
        mode,
        platform: { os: "linux", arch: "x64" },
        fixtureVersion: liveContract.fixtureVersion,
        fixtureSha256: checkout.fixtureSha256,
        runnerSha256: checkout.runnerSha256,
        packageVersion: checkout.version,
        runtimePackageVersion: checkout.version,
        honoPackageVersion: liveContract.toolchain.honoVersion,
        esbuildVersion: liveContract.toolchain.esbuildVersion,
        nodeVersion: liveContract.toolchain.nodeVersion,
        zlibVersion: liveContract.toolchain.zlibVersion,
        compression: liveContract.compression,
        repeats: 2,
        pnpmLockSha256: checkout.pnpmLockSha256,
        dependencyFingerprint: {
          policyVersion: checkout.dependencyFingerprint.policyVersion,
          sha256: checkout.dependencyFingerprint.sha256,
        },
        cargoLockSha256: checkout.cargoLockSha256,
      };
      edit?.(provenance, mode);
      return provenance;
    });
    tamper?.(dir);
    return dir;
  }

  // The live Linux contract re-pinned to `checkout` at the live (policy 1) fingerprint.
  function pinnedAt(checkout, artifactDir) {
    const contract = structuredClone(liveContract);
    contract.ceilings = ceilingsOf(artifactDir);
    contract.fixtureSha256 = { ...checkout.fixtureSha256 };
    contract.runnerSha256 = { ...checkout.runnerSha256 };
    contract.toolchain.packageVersion = checkout.version;
    contract.toolchain.pnpmLockSha256 = checkout.pnpmLockSha256;
    contract.toolchain.cargoLockSha256 = checkout.cargoLockSha256;
    contract.dependencyFingerprint = { ...checkout.priorFingerprint };
    return contract;
  }

  beforeAll(() => {
    repo = makePolicyCheckout(roots, { editHelper: policyHelper.v2 });
    artifact = artifactAt(repo);
    pinned = pinnedAt(repo, artifact);
    writeFileSync(at(CONTRACT), contractText(pinned));
    repo.git("add", "-A");
    repo.git("commit", "-qm", "test fixture: policy-1 contract, policy-2 helper copy");
    saved = Object.fromEntries(OUTPUTS.map((file) => [file, read(file)]));
  });
  afterEach(() => {
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const argv = (extra = {}) =>
    Object.entries({
      artifact,
      "run-id": "91",
      "job-id": "92",
      "head-sha": HEAD,
      "source-sha": SOURCE,
      reason: "fingerprint-policy",
      stage: join(tempDir("zfb-policy-stage-"), "stage"),
      ...extra,
    })
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    return vi.spyOn(console, "error").mockImplementation(() => {});
  };
  const unchanged = (before) =>
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));
  const identityLine = (fingerprint) =>
    "Policy identity: `policyVersion: " +
    fingerprint.policyVersion +
    "`, `parser: " +
    JSON.stringify(fingerprint.parser) +
    "`";

  it("re-pins the four fingerprint keys with a one-step policyVersion increment end to end", async () => {
    quiet();
    expect(LIVE_POLICY_VERSION).toBe(1);
    expect(repo.priorFingerprint.policyVersion).toBe(1);
    expect(repo.dependencyFingerprint.policyVersion).toBe(2);
    const stage = join(tempDir("zfb-policy-stage-"), "stage");
    await main(argv({ stage }), repo.root);
    unchanged(OUTPUTS.map((file) => saved[file]));
    landStaged(stage, repo.root);
    const next = written();
    const moved = contractDiffKeys(pinned, next);
    expect(moved.filter((key) => !key.startsWith("baselineEvidence."))).toEqual([
      "baselineSourceSha",
      "dependencyFingerprint.helperSha256",
      "dependencyFingerprint.policyVersion",
      "dependencyFingerprint.sha256",
      "outcome",
      "platformStatus",
    ]);
    expect(() =>
      assertAllowedMovement("fingerprint-policy", FINGERPRINT_POLICY_MOVEMENT, pinned, next),
    ).not.toThrow();
    expect(next.dependencyFingerprint).toEqual(repo.dependencyFingerprint);
    expect(next.ceilings).toEqual(pinned.ceilings);
    expect(next.toolchain).toEqual(pinned.toolchain);
    expect(next.outcome).toBe("fingerprint-policy");
    expect(next.baselineSourceSha).toBe(HEAD);
    expect(next.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 91,
      jobId: 92,
      sourceSha: SOURCE,
    });
    expect(next.baselineEvidence.fingerprintPolicyTransition).toEqual({
      from: repo.priorFingerprint,
      to: repo.dependencyFingerprint,
    });
    expect(next.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 3,
      outcome: pinned.outcome,
      packageVersion: pinned.toolchain.packageVersion,
      baselineSourceSha: pinned.baselineSourceSha,
      baselineRunId: pinned.baselineEvidence.runId,
      contract: {
        sha256: createHash("sha256").update(saved[CONTRACT]).digest("hex"),
        commit: repo.git("rev-parse", "HEAD"),
        path: CONTRACT,
      },
    });
    expect(next.platformStatus).toContain(
      "parser and helperSha256 are proven against the checkout helper, not attested by the artifact",
    );
    // The inventory is the fixture helper's own --explain output, byte for byte.
    expect(read(INVENTORY).equals(helperExplainBytes(repo.root))).toBe(true);
    expect(JSON.parse(read(INVENTORY).toString("utf8")).policyVersion).toBe(2);
    expect(read(POLICY).toString("utf8")).toBe(
      saved[POLICY]
        .toString("utf8")
        .replace(identityLine(repo.priorFingerprint), identityLine(next.dependencyFingerprint)),
    );
    expect(read(POLICY).toString("utf8")).toContain("Policy identity: `policyVersion: 2`");
    const readme = read(README).toString("utf8");
    expect(readme.startsWith(saved[README].toString("utf8"))).toBe(true);
    expect(readme).toContain("## v" + repo.version + " fingerprint-policy re-pin");
    expect(readme).toContain("[workflow run 91]");
    expect(readme).toContain("The Darwin arm64 contract moves in the same paired change");
    expect(read(TEST).toString("utf8")).toBe(rewriteTestPins(saved[TEST].toString("utf8"), next));
  });

  it("stages a changed canonical form and parser identity outside the checkout", async () => {
    quiet();
    const other = makePolicyCheckout(roots, { editHelper: policyHelper.v2Canonical });
    const otherArtifact = artifactAt(other);
    const otherPinned = pinnedAt(other, otherArtifact);
    writeFileSync(join(other.root, CONTRACT), contractText(otherPinned));
    const live = OUTPUTS.map((file) => readFileSync(join(other.root, file)));
    expect(other.dependencyFingerprint.parser).toBe("yaml@2.9.0+fixture");
    const stage = join(tempDir("zfb-policy-stage-"), "stage");
    await main(argv({ artifact: otherArtifact, stage }), other.root);
    OUTPUTS.forEach((file, i) =>
      expect(readFileSync(join(other.root, file)).equals(live[i])).toBe(true),
    );
    const staged = (file) => readFileSync(join(stage, file));
    const next = JSON.parse(staged(CONTRACT).toString("utf8"));
    const moved = contractDiffKeys(otherPinned, next);
    expect(moved.filter((key) => key.startsWith("dependencyFingerprint."))).toEqual([
      "dependencyFingerprint.helperSha256",
      "dependencyFingerprint.parser",
      "dependencyFingerprint.policyVersion",
      "dependencyFingerprint.sha256",
    ]);
    expect(() =>
      assertAllowedMovement("fingerprint-policy", FINGERPRINT_POLICY_MOVEMENT, otherPinned, next),
    ).not.toThrow();
    expect(next.dependencyFingerprint).toEqual(other.dependencyFingerprint);
    expect(staged(INVENTORY).equals(helperExplainBytes(other.root))).toBe(true);
    expect(staged(POLICY).toString("utf8")).toContain(
      'Policy identity: `policyVersion: 2`, `parser: "yaml@2.9.0+fixture"`',
    );
    const manifest = JSON.parse(staged("stage-manifest.json").toString("utf8"));
    expect(Object.keys(manifest.destinations)).toEqual([...OUTPUTS].sort());
    expect(manifest.runs.map((run) => run.reason)).toEqual(["fingerprint-policy"]);
    expect(() => applyStage(stage, other.root)).toThrow(
      "--reason fingerprint-policy is not staged for darwin-arm64",
    );
    OUTPUTS.forEach((file, i) =>
      expect(readFileSync(join(other.root, file)).equals(live[i])).toBe(true),
    );
  });

  it("refuses to run without --stage, in parseArgs, before reading or writing anything", async () => {
    const message =
      "--reason fingerprint-policy re-pins both platforms together and only stages: pass --stage <dir> for Linux x64 and Darwin arm64 into the same directory, then --apply <dir> (nothing was written)";
    expect(() => parseArgs(argv({ stage: undefined }))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(argv({ stage: undefined }), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });

  it("round-trips an escaped parser identity from a fixture helper through successive re-pins", async () => {
    quiet();
    const other = makePolicyCheckout(roots, { editHelper: policyHelper.v2Escaped });
    expect(other.dependencyFingerprint.parser).toBe('yaml@2.9.0+"q"\\$&');
    const otherArtifact = artifactAt(other);
    writeFileSync(join(other.root, CONTRACT), contractText(pinnedAt(other, otherArtifact)));
    const livePolicy = readFileSync(join(other.root, POLICY), "utf8");
    const stage = join(tempDir("zfb-policy-stage-"), "stage");
    await main(argv({ artifact: otherArtifact, stage }), other.root);
    const staged = readFileSync(join(stage, POLICY), "utf8");
    const line = identityLine(other.dependencyFingerprint);
    expect(line).toContain('`parser: "yaml@2.9.0+\\"q\\"\\\\$&"`');
    expect(staged).toBe(livePolicy.replace(identityLine(other.priorFingerprint), () => line));
    // The next re-pin finds and replaces the escaped line this one wrote.
    expect(rewritePolicyIdentity(staged, other.dependencyFingerprint)).toBe(staged);
    const later = { policyVersion: 3, parser: "yaml@3.0.0" };
    expect(rewritePolicyIdentity(staged, later)).toBe(
      livePolicy.replace(identityLine(other.priorFingerprint), () => identityLine(later)),
    );
  });

  it("refuses a helper that exports a version other than the one it computes", async () => {
    quiet();
    const other = makePolicyCheckout(roots, { editHelper: policyHelper.exports3 });
    expect(other.dependencyFingerprint.policyVersion).toBe(2);
    const otherArtifact = artifactAt(other);
    writeFileSync(join(other.root, CONTRACT), contractText(pinnedAt(other, otherArtifact)));
    const live = OUTPUTS.map((file) => readFileSync(join(other.root, file)));
    const stage = tempDir("zfb-policy-stage-mismatch-");
    await expect(main(argv({ artifact: otherArtifact, stage }), other.root)).rejects.toThrow(
      "the checkout's policyVersion 2 does not match the helper's exported FINGERPRINT_POLICY_VERSION 3",
    );
    expect(readdirSync(stage)).toEqual([]);
    OUTPUTS.forEach((file, i) =>
      expect(readFileSync(join(other.root, file)).equals(live[i])).toBe(true),
    );
  });

  it("keeps a flat predecessor across successive dependency-graph and fingerprint-policy re-pins", async () => {
    quiet();
    // The dependency-graph re-pin that preceded this policy change, from a
    // synthetic earlier graph to the live (policy 1) fingerprint.
    const earlier = structuredClone(pinned);
    earlier.dependencyFingerprint.sha256 = "5".repeat(64);
    const graph = rebaselineContract(
      earlier,
      totalsFrom(earlier),
      {
        version: repo.version,
        pnpmLockSha256: repo.pnpmLockSha256,
        normalizerSha256: earlier.normalizerSha256,
        sourceSha: SOURCE,
        headSha: HEAD,
        runId: 81,
        jobId: 82,
        runLabel: "Dependency-graph re-pin run",
        identity: {
          fixtureSha256: repo.fixtureSha256,
          runnerSha256: repo.runnerSha256,
          cargoLockSha256: repo.cargoLockSha256,
          toolchain: {
            honoVersion: earlier.toolchain.honoVersion,
            esbuildVersion: earlier.toolchain.esbuildVersion,
            nodeVersion: earlier.toolchain.nodeVersion,
            zlibVersion: earlier.toolchain.zlibVersion,
          },
        },
        artifactFingerprint: {
          policyVersion: repo.priorFingerprint.policyVersion,
          sha256: repo.priorFingerprint.sha256,
        },
        dependencyFingerprint: repo.priorFingerprint,
        previousContract: { sha256: "d".repeat(64) },
      },
      "dependency-graph",
    ).contract;
    expect(graph.outcome).toBe("dependency-graph-repin");
    expect(graph.baselineEvidence.repinnedFrom.outcome).toBe(pinned.outcome);
    writeFileSync(at(CONTRACT), contractText(graph));
    const stage = join(tempDir("zfb-policy-stage-"), "stage");
    await main(argv({ stage }), repo.root);
    landStaged(stage, repo.root);
    const next = written();
    expect(next.outcome).toBe("fingerprint-policy");
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.baselineEvidence.repinnedFrom).toMatchObject({
      outcome: "dependency-graph-repin",
      baselineSourceSha: HEAD,
      baselineRunId: 81,
    });
    expect(JSON.stringify(next.baselineEvidence.repinnedFrom)).not.toContain(pinned.outcome);
    expect(next.baselineEvidence).not.toHaveProperty("fingerprintTransition");
    expect(next.baselineEvidence.fingerprintPolicyTransition.from).toEqual(
      graph.dependencyFingerprint,
    );
  });

  it("is refused by dependency-graph, which names fingerprint-policy", async () => {
    quiet();
    const before = OUTPUTS.map(read);
    await expect(main(argv({ reason: "dependency-graph" }), repo.root)).rejects.toThrow(
      /moves only dependencyFingerprint\.sha256, but policyVersion, helperSha256 differ from the contract; .*`fingerprint-policy` \(#4218\)/,
    );
    unchanged(before);
  });

  // Each refusal leaves the contract, the budget test, the README, the
  // inventory and the policy document unchanged.
  const fingerprint = (name, edit, message) => ({
    name,
    contract: (c) => edit(c.dependencyFingerprint),
    message,
  });
  const identity = (name, edit) => ({
    name: "a changed " + name,
    contract: edit,
    message:
      "--reason fingerprint-policy moves only the dependency fingerprint, together with a policyVersion step, but " +
      name +
      " differ from the contract",
  });
  const sidecar = (mode, edit) => (dir) => {
    const path = join(dir, "real-" + mode, "measurement.normalized.json");
    const value = JSON.parse(readFileSync(path, "utf8"));
    edit(value);
    writeFileSync(path, JSON.stringify(value));
  };
  it.each([
    fingerprint(
      "+0 (the contract already at policy 2)",
      (f) => Object.assign(f, repo.dependencyFingerprint, { sha256: repo.priorFingerprint.sha256 }),
      "--reason fingerprint-policy: policyVersion did not move; use fingerprint-helper or dependency-graph",
    ),
    fingerprint("+2", (f) => (f.policyVersion = 0), "policyVersion must increase by exactly one"),
    fingerprint("-1", (f) => (f.policyVersion = 3), "policyVersion must increase by exactly one"),
    fingerprint(
      "a parser move without a version step",
      (f) => Object.assign(f, repo.dependencyFingerprint, { parser: "yaml@2.8.0" }),
      "parser moved from yaml@2.8.0 to yaml@2.9.0 without a policyVersion step",
    ),
    fingerprint(
      "a helperSha256 move without a version step",
      (f) => Object.assign(f, repo.dependencyFingerprint, { helperSha256: "1".repeat(64) }),
      "helperSha256 moved without a policyVersion step; a helper-only edit needs `fingerprint-helper` (#4198)",
    ),
    {
      name: "an artifact at policyVersion 1 against the policy-2 checkout",
      provenance: (p) => (p.dependencyFingerprint.policyVersion = 1),
      message:
        "workspace artifact dependency fingerprint policyVersion differs from this checkout's",
    },
    {
      name: "an artifact fingerprint that is not the checkout's",
      provenance: (p, mode) => {
        if (mode === "packed") p.dependencyFingerprint.sha256 = repo.priorFingerprint.sha256;
      },
      message: "packed artifact dependency fingerprint sha256 differs from this checkout's",
    },
    {
      name: "raw +1",
      contract: (c) => (c.ceilings.workspace.model.raw -= 1),
      message: "raw totals changed (workspace/model); this is not a fingerprint-policy re-pin",
    },
    {
      name: "raw -1",
      contract: (c) => (c.ceilings.packed.model.raw += 1),
      message: "raw totals changed (packed/model)",
    },
    {
      name: "gzipNormalized +1",
      contract: (c) => (c.ceilings.workspace["show-for"].gzipNormalized -= 1),
      message: "gzipNormalized totals changed (workspace/show-for +1)",
    },
    {
      name: "gzipNormalized -1",
      contract: (c) => (c.ceilings.packed["json-api"].gzipNormalized += 1),
      message: "gzipNormalized totals changed (packed/json-api -1)",
    },
    {
      name: "a changed Cargo.lock",
      contract: (c) => (c.toolchain.cargoLockSha256 = "0".repeat(64)),
      message:
        /--reason fingerprint-policy cannot move Cargo\.lock .*needs --reason cargo-producer-lock first/,
    },
    identity("packageVersion", (c) => (c.toolchain.packageVersion = "0.0.1")),
    ...["honoVersion", "esbuildVersion", "nodeVersion", "zlibVersion"].map((key) =>
      identity("toolchain." + key, (c) => (c.toolchain[key] = "0.0.0-test")),
    ),
    {
      name: "a changed fixture set",
      contract: (c) => (c.ceilings.workspace.extra = { raw: 1, gzipNormalized: 1 }),
      message: "fixture set differs from the contract (workspace/extra)",
    },
    identity("fixtureSha256", (c) => (c.fixtureSha256.model = "0".repeat(64))),
    identity("runnerSha256", (c) => (c.runnerSha256["measure.mjs"] = "0".repeat(64))),
    identity("normalizerSha256", (c) => (c.normalizerSha256 = "e".repeat(64))),
    identity("allowance", (c) => (c.allowance = { raw: 1, gzipNormalized: 0 })),
    identity("metric", (c) => (c.metric = "gzip")),
    {
      name: "a schemaVersion 2 contract",
      contract: (c) => {
        delete c.dependencyFingerprint;
        c.schemaVersion = 2;
      },
      message:
        "--reason fingerprint-policy needs a schemaVersion 3 contract, found schemaVersion 2",
    },
    {
      name: "a wrong-platform artifact",
      provenance: (p) => (p.platform = { os: "darwin", arch: "arm64" }),
      message: "not linux-x64; Darwin arm64 uses rebaseline-island-size-darwin.mjs",
    },
    {
      name: "a tampered sidecar",
      tamper: sidecar("packed", (s) => (s.results.model.shipped.gzipNormalized -= 1)),
      message:
        "real-packed: measurement.normalized.json results disagree with the recomputation from the saved bytes",
    },
    {
      name: "a stale sidecar",
      tamper: sidecar("workspace", (s) => (s.source.sha256 = "0".repeat(64))),
      message: "(stale sidecar)",
    },
    {
      name: "a sidecar whose normalizer.sha256 was tampered",
      tamper: sidecar("workspace", (s) => (s.normalizer.sha256 = "0".repeat(64))),
      message:
        "real-workspace: measurement.normalized.json normalizer.sha256 differs from the current normalizer; refusing",
    },
    {
      name: "two policy identity lines",
      setup: () =>
        writeFileSync(
          at(POLICY),
          saved[POLICY].toString("utf8") +
            '\nPolicy identity: `policyVersion: 1`, `parser: "yaml@2.9.0"`.\n',
        ),
      message: "2 policy identity lines in LOCK-FINGERPRINT-POLICY.md; expected exactly one",
    },
    {
      name: "a --source-sha that is not the artifact's",
      args: { "source-sha": "d".repeat(40) },
      message: "workspace artifact measured source " + SOURCE + ", not --source-sha",
    },
    {
      name: "a cargo-only flag",
      args: { "expected-old-cargo-sha": "0".repeat(64) },
      message: "--expected-old-cargo-sha is only for --reason cargo-producer-lock",
    },
    {
      name: "a missing policy identity line",
      setup: () =>
        writeFileSync(
          at(POLICY),
          saved[POLICY].toString("utf8").replace("Policy identity:", "Identity:"),
        ),
      message: "policy identity line not found in LOCK-FINGERPRINT-POLICY.md",
    },
    {
      name: "a missing test-pin anchor",
      setup: () =>
        writeFileSync(
          at(TEST),
          saved[TEST].toString("utf8").replace("records the reviewed Linux x64 ceilings", "x"),
        ),
      message: "Linux ceiling test not found",
    },
    {
      name: "a missing packed measurement.json",
      tamper: (dir) => rmSync(join(dir, "real-packed", "measurement.json")),
      message: "artifact is missing real-packed/measurement.json",
    },
    {
      name: "a missing workspace measurement.normalized.json",
      tamper: (dir) => rmSync(join(dir, "real-workspace", "measurement.normalized.json")),
      message: "real-workspace: missing measurement.normalized.json; refusing",
    },
  ])(
    "refuses $name and writes nothing, staged or live",
    async ({ contract: edit, provenance, tamper, args, setup, message }) => {
      quiet();
      if (edit) {
        const contract = structuredClone(pinned);
        edit(contract);
        writeFileSync(at(CONTRACT), contractText(contract));
      }
      setup?.();
      const dir = provenance || tamper ? artifactAt(repo, provenance, tamper) : artifact;
      const before = OUTPUTS.map(read);
      const stage = tempDir("zfb-policy-refused-stage-");
      await expect(main(argv({ artifact: dir, stage, ...args }), repo.root)).rejects.toThrow(
        message,
      );
      expect(readdirSync(stage)).toEqual([]);
      unchanged(before);
    },
  );

  it.each([
    ["an unknown --reason", { reason: "fingerprint-policies" }, /--reason must be one of /],
    ["an empty --reason", { reason: "" }, "required --reason"],
    ["a missing --source-sha", { "source-sha": undefined }, "required --source-sha"],
    ["a malformed --source-sha", { "source-sha": "abc" }, /--source-sha must be a full 40/],
  ])("refuses %s in parseArgs, before reading anything", async (_name, extra, message) => {
    const missing = join(tmpdir(), "zfb-no-such-artifact-" + process.pid);
    expect(() => parseArgs(argv({ artifact: missing, ...extra }))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(argv({ artifact: missing, ...extra }), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });
});

// cargo-internal-rewire (#4258). Test fixture — synthetic internal rewire, not
// native evidence: five-record locks whose `zfb-render` record drops `sha2`
// and gains the new unpublished `zfb-js-host` record (fixtures/island-size-
// cargo-rewire.mjs). Pure proofs first, then end-to-end CLI runs against one
// git-backed checkout whose tag `old-lock` holds the OLD Cargo.lock and whose
// HEAD holds the NEW lock, the pinned contract and the workspace manifests.
describe("cargo-internal-rewire (#4258)", () => {
  const roots = [];
  const { oldText, newText, OLD, NEW } = rewireLocks();
  const SOURCE = { commit: "a".repeat(40), path: "Cargo.lock", sha256: OLD };
  const NEW_SOURCE = { sha256: NEW };
  const DECLARATION_SOURCE = {
    path: "research/v3-island-size/rewire.json",
    sha256: "1".repeat(64),
  };
  const manifests = (edit = (_path, text) => text) => {
    const files = Object.fromEntries(
      Object.entries(REWIRE_MANIFESTS).map(([path, text]) => [path, edit(path, text)]),
    );
    return (path) => {
      if (!Object.hasOwn(files, path)) throw new Error("ENOENT " + path);
      return Buffer.from(files[path]);
    };
  };
  const closures = (target = LINUX_TARGET, edits = {}) => ({
    old: { receipt: closureReceipt("old", target, edits.old), sha256: "2".repeat(64) },
    new: { receipt: closureReceipt("new", target, edits.new), sha256: "3".repeat(64) },
  });
  const inputs = (edit = (i) => i) =>
    edit({
      contractSha: OLD,
      artifactSha: NEW,
      expectedOldSha: OLD,
      expectedNewSha: NEW,
      oldText,
      newText,
      oldLockSource: SOURCE,
      newLockSource: NEW_SOURCE,
      declaration: rewireDeclaration(),
      declarationSource: DECLARATION_SOURCE,
      closures: closures(),
      target: LINUX_TARGET,
      readManifest: manifests(),
    });
  const prove = (edit) => proveCargoInternalRewire(inputs(edit));
  // A new lock the checkout and the artifact both carry, with the declaration
  // re-bound to its digest so only the named problem remains.
  const withNewLock =
    (text, declare = () => {}) =>
    (i) => ({
      ...i,
      newText: text,
      expectedNewSha: rewireSha(text),
      artifactSha: rewireSha(text),
      newLockSource: { sha256: rewireSha(text) },
      declaration: rewireDeclaration((d) => {
        d.newCargoLockSha256 = rewireSha(text);
        declare(d);
      }),
      closures: closures(LINUX_TARGET, { new: (r) => (r.cargoLockSha256 = rewireSha(text)) }),
    });
  afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    vi.restoreAllMocks();
  });

  it("is a paired, source-bound reason that regenerates no inventory", () => {
    expect(PAIRED_REASONS).toContain("cargo-internal-rewire");
    expect(SOURCE_BOUND_REASONS).toContain("cargo-internal-rewire");
    expect(INVENTORY_REASONS).toEqual(["dependency-graph", "fingerprint-policy"]);
    expect(CARGO_INTERNAL_REWIRE_MOVEMENT).toEqual(CARGO_PRODUCER_LOCK_MOVEMENT);
    expect(REWIRE_ARGS).toEqual([
      "rewire-declaration",
      "old-closure",
      "new-closure",
      "new-cargo-ref",
    ]);
  });

  it("splits a record into its edges and every other line, and resolves entries exactly", () => {
    const render = recordOf(newText, "zfb-render@0.0.0");
    expect(parseLockRecord(render.body, "x")).toEqual({
      dependencies: ["anyhow", "zfb-js-host"],
      rest: ["[[package]]", 'name = "zfb-render"', 'version = "0.0.0"'],
      source: undefined,
      checksum: undefined,
    });
    const anyhow = recordOf(newText, "anyhow@1.0.100");
    expect(parseLockRecord(anyhow.body, "x")).toMatchObject({
      dependencies: null,
      source: 'source = "registry+https://github.com/rust-lang/crates.io-index"',
    });
    expect(() => parseLockRecord(render.body.replace(/\]\n$/, ""), "x")).toThrow(
      /unterminated dependencies list/,
    );
    expect(() => parseLockRecord(render.body.replace(' "anyhow",', " anyhow,"), "x")).toThrow(
      /unreadable dependency line/,
    );
    expect(() => parseLockRecord(render.body.slice(0, -1), "x")).toThrow(/end with a newline/);
    const records = parseCargoLock(newText, "x").packages.map((record) => {
      const [name, version] = record.id.split("@");
      return { ...record, name, version, fields: parseLockRecord(record.body, "x") };
    });
    expect(resolveDependencyEntry("sha2", records).record.id).toBe("sha2@0.10.9");
    expect(resolveDependencyEntry("sha2 0.10.9", records).record.id).toBe("sha2@0.10.9");
    expect(
      resolveDependencyEntry(
        "sha2 0.10.9 (registry+https://github.com/rust-lang/crates.io-index)",
        records,
      ).record.id,
    ).toBe("sha2@0.10.9");
    expect(resolveDependencyEntry("sha3", records).problem).toMatch(/names no record/);
    expect(resolveDependencyEntry("sha2 0.10.8", records).problem).toMatch(/names no record/);
    const twoSha2 = [...records, { ...records[1], id: "sha2@0.10.10", version: "0.10.10" }];
    expect(resolveDependencyEntry("sha2", twoSha2).problem).toMatch(/is ambiguous/);
    expect(resolveDependencyEntry("sha2 0.10.10", twoSha2).record.version).toBe("0.10.10");
    expect(resolveDependencyEntry("bad entry here now", records).problem).toMatch(/malformed/);
  });

  it.each(['"""', "'''"])("refuses metadata hidden in %s multiline strings", (delimiter) => {
    const text =
      '[package]\nname = "x"\nversion = "0.0.0"\ndescription = ' +
      delimiter +
      "\npublish = false\n" +
      delimiter +
      "\n";
    expect(() => readManifestPackage(text)).toThrow(/unsupported multiline TOML strings/);
    expect(() =>
      workspaceMemberProblem(
        "description = " + delimiter + '\n[workspace]\nmembers = ["crates/*"]\n' + delimiter + "\n",
        "crates/x/Cargo.toml",
      ),
    ).toThrow(/unsupported multiline TOML strings/);
  });

  it("ignores TOML comments without truncating quoted hashes", () => {
    expect(
      readManifestPackage('[package] # comment\nname = "x#y" # comment\npublish = false # """\n'),
    ).toEqual({ name: "x#y", publish: false });
    expect(
      workspaceMemberProblem(
        '[workspace]\nmembers = [\n"crates/zfb", # "crates/zfb-js-host"\n]\n',
        "crates/zfb-js-host/Cargo.toml",
      ),
    ).toMatch(/not a workspace member path/);
    expect(
      workspaceMemberProblem(
        '[workspace]\nmembers = ["crates/*"]\nexclude = [\n# "crates/zfb"\n]\n',
        "crates/zfb/Cargo.toml",
      ),
    ).toBeNull();
  });

  it("reads the [package] table and workspace membership from manifests", () => {
    expect(readManifestPackage(REWIRE_MANIFESTS["crates/zfb-js-host/Cargo.toml"])).toEqual({
      name: "zfb-js-host",
      version: "0.0.0",
      edition: "2021",
      publish: false,
    });
    expect(readManifestPackage('[dependencies]\nserde = "1"\n')).toBeNull();
    expect(
      readManifestPackage('[package]\nname = "x" # comment\nversion.workspace = true\n[lib]\n'),
    ).toEqual({ name: "x" });
    expect(
      workspaceMemberProblem(REWIRE_MANIFESTS["Cargo.toml"], "crates/zfb-render/Cargo.toml"),
    ).toBeNull();
    expect(workspaceMemberProblem(REWIRE_MANIFESTS["Cargo.toml"], "tools/x/Cargo.toml")).toBe(
      "tools/x/Cargo.toml is not a workspace member path",
    );
    expect(workspaceMemberProblem(REWIRE_MANIFESTS["Cargo.toml"], "crates/a/b/Cargo.toml")).toMatch(
      /not a workspace member path/,
    );
    expect(workspaceMemberProblem("[workspace]\n", "crates/x/Cargo.toml")).toMatch(/lists no/);
    expect(workspaceMemberProblem('members = ["crates/*"]\n', "crates/x/Cargo.toml")).toMatch(
      /has no \[workspace\] table/,
    );
    expect(
      workspaceMemberProblem('[workspace]\nmembers = ["crates/zfb"]\n', "crates/zfb/Cargo.toml"),
    ).toBeNull();
    // A `members` key in another table never counts; only the [workspace] table does.
    expect(
      workspaceMemberProblem(
        '[package.metadata.x]\nmembers = ["evil/*"]\n\n[workspace]\nmembers = ["crates/*"]\n',
        "evil/x/Cargo.toml",
      ),
    ).toMatch(/not a workspace member path/);
    expect(
      workspaceMemberProblem(
        '[workspace]\nmembers = [\n  "crates/*",\n]\nexclude = ["crates/old"]\n',
        "crates/old/Cargo.toml",
      ),
    ).toBe("crates/old/Cargo.toml is a workspace-excluded path");
    expect(lockSourceValue(undefined)).toBeNull();
    expect(
      lockSourceValue('source = "registry+https://github.com/rust-lang/crates.io-index"'),
    ).toBe("registry+https://github.com/rust-lang/crates.io-index");
    expect(
      lockSourceValue('source = "git+https://example.invalid/x?rev=abc#' + "0".repeat(40) + '"'),
    ).toBe("git+https://example.invalid/x?rev=abc");
  });

  it("normalizes a declaration and refuses a malformed one", () => {
    const normalized = checkRewireDeclaration(rewireDeclaration());
    expect(normalized.closure).toEqual({ added: ["zfb-js-host@0.0.0"], changed: [] });
    expect(normalized.changed[0]).toMatchObject({
      removedDependencies: ["sha2"],
      addedDependencies: ["zfb-js-host"],
    });
    for (const [name, edit, message] of [
      ["a wrong schema", (d) => (d.schemaVersion = 3), /schemaVersion must be 1 or 2/],
      [
        "no producer",
        (d) => delete d.producer,
        /producer must be zfb, the measured CLI package, not undefined/,
      ],
      [
        "another producer",
        (d) => (d.producer = "zudo-wind"),
        /producer must be zfb, the measured CLI package, not "zudo-wind"/,
      ],
      ["a short hash", (d) => (d.oldCargoLockSha256 = "abc"), /oldCargoLockSha256 must be/],
      ["equal hashes", (d) => (d.newCargoLockSha256 = d.oldCargoLockSha256), /no Cargo\.lock move/],
      [
        "no records",
        (d) => ((d.added = []), (d.changed = [])),
        /declares no added or changed record/,
      ],
      ["a bad id", (d) => (d.added[0].id = "zfb-js-host"), /id of the form name@version/],
      [
        "a .. manifest",
        (d) => (d.added[0].manifest = "../x/Cargo.toml"),
        /must not contain a \.\. segment/,
      ],
      [
        "a non-manifest path",
        (d) => (d.added[0].manifest = "crates/x/lib.rs"),
        /must be a Cargo\.toml path/,
      ],
      ["no added sha", (d) => delete d.added[0].sha256, /needs sha256/],
      [
        "identical changed hashes",
        (d) => (d.changed[0].newSha256 = d.changed[0].oldSha256),
        /identical old and new bytes/,
      ],
      [
        "an empty edge delta",
        (d) => ((d.changed[0].removedDependencies = []), (d.changed[0].addedDependencies = [])),
        /declares no dependency-edge delta/,
      ],
      ["a repeated id", (d) => d.changed.push({ ...d.changed[0] }), /repeats zfb-render@0\.0\.0/],
      [
        "an undeclared closure id",
        (d) => (d.closure.added = ["anyhow@1.0.100"]),
        /closure names anyhow@1\.0\.100, which is not declared/,
      ],
      ["no closure", (d) => delete d.closure, /closure must carry/],
    ]) {
      expect(() => checkRewireDeclaration(rewireDeclaration(edit)), name).toThrow(message);
    }
    expect(() => checkRewireDeclaration("x")).toThrow(/must be a JSON object/);
  });

  it("proves the declared rewire and records exact bytes, edges, sources and closure rows", () => {
    const transition = prove();
    const added = recordOf(newText, "zfb-js-host@0.0.0");
    expect(transition).toEqual({
      oldSha: OLD,
      newSha: NEW,
      oldLockSource: SOURCE,
      newLockSource: NEW_SOURCE,
      declaration: DECLARATION_SOURCE,
      producer: "zfb",
      manifestsAtCommit: null,
      added: [
        {
          id: "zfb-js-host@0.0.0",
          manifest: "crates/zfb-js-host/Cargo.toml",
          manifestSha256: rewireSha(REWIRE_MANIFESTS["crates/zfb-js-host/Cargo.toml"]),
          sha256: rewireSha(added.body),
          dependencies: ["anyhow", "sha2"],
        },
      ],
      changed: [
        {
          id: "zfb-render@0.0.0",
          manifest: "crates/zfb-render/Cargo.toml",
          manifestSha256: rewireSha(REWIRE_MANIFESTS["crates/zfb-render/Cargo.toml"]),
          oldSha256: rewireSha(recordOf(oldText, "zfb-render@0.0.0").body),
          newSha256: rewireSha(recordOf(newText, "zfb-render@0.0.0").body),
          removedDependencies: ["sha2"],
          addedDependencies: ["zfb-js-host"],
        },
      ],
      removed: [],
      closure: {
        target: LINUX_TARGET,
        oldSha256: "2".repeat(64),
        newSha256: "3".repeat(64),
        rows: { old: 5, new: 6, unchanged: 5 },
        added: [{ id: "zfb-js-host@0.0.0", features: ["default", "services"] }],
        changed: [],
      },
    });
    // Raw bytes are hashed before decoding: Buffers prove like their text.
    expect(
      prove((i) => ({ ...i, oldText: Buffer.from(oldText), newText: Buffer.from(newText) })).newSha,
    ).toBe(NEW);
    // A cited new-lock commit is carried through verbatim.
    const cited = { sha256: NEW, commit: "b".repeat(40), path: "Cargo.lock" };
    expect(prove((i) => ({ ...i, newLockSource: cited })).newLockSource).toEqual(cited);
    expect(prove((i) => ({ ...i, manifestCommit: "b".repeat(40) })).manifestsAtCommit).toBe(
      "b".repeat(40),
    );
    expect(() => prove((i) => ({ ...i, manifestCommit: "abc" }))).toThrow(
      /manifestCommit must be a full commit SHA or null/,
    );
  });

  const external = (text) =>
    text.replace('name = "sha2"\nversion = "0.10.9"', 'name = "sha2"\nversion = "0.10.10"');
  it.each([
    [
      "a wrong old hash",
      (i) => ({ ...i, expectedOldSha: "1".repeat(64) }),
      /the contract pins Cargo\.lock [0-9a-f]{64}, not --expected-old-cargo-sha/,
    ],
    [
      "an old blob that is not the pinned lock",
      (i) => ({
        ...i,
        oldText: oldText + "\n",
        oldLockSource: { ...SOURCE, sha256: rewireSha(oldText + "\n") },
      }),
      /hashes to [0-9a-f]{64}, not --expected-old-cargo-sha/,
    ],
    [
      "a wrong new hash",
      (i) => ({ ...i, expectedNewSha: "2".repeat(64) }),
      /the checkout's Cargo\.lock hashes to [0-9a-f]{64}, not --expected-new-cargo-sha/,
    ],
    [
      "an artifact measured at another lock",
      (i) => ({ ...i, artifactSha: "3".repeat(64) }),
      /the artifact measured Cargo\.lock 3{64}, not --expected-new-cargo-sha/,
    ],
    [
      "equal old and new hashes",
      (i) => ({ ...i, expectedNewSha: OLD }),
      /equals --expected-new-cargo-sha; no Cargo\.lock move/,
    ],
    [
      "a filesystem old-lock source",
      (i) => ({ ...i, oldLockSource: "old.lock" }),
      /must be its verified git blob \{commit, path, sha256\}/,
    ],
    [
      "a short old-lock commit",
      (i) => ({ ...i, oldLockSource: { ...SOURCE, commit: "abc" } }),
      /must be its verified git blob/,
    ],
    [
      "a new-lock reference with a wrong digest",
      (i) => ({ ...i, newLockSource: { sha256: "4".repeat(64) } }),
      /new Cargo\.lock source digest is not the checkout's/,
    ],
    [
      "a malformed new-lock reference",
      (i) => ({ ...i, newLockSource: { sha256: NEW, commit: "abc", path: "Cargo.lock" } }),
      /new Cargo\.lock source must be \{sha256\[, commit, path\]\}/,
    ],
    [
      "a declaration without a source digest",
      (i) => ({ ...i, declarationSource: { path: "x" } }),
      /declaration source must record its path and sha256/,
    ],
    [
      "a declaration bound to another old lock",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.oldCargoLockSha256 = "5".repeat(64))),
      }),
      /declaration's oldCargoLockSha256 is not --expected-old-cargo-sha/,
    ],
    [
      "a declaration bound to another new lock",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.newCargoLockSha256 = "5".repeat(64))),
      }),
      /declaration's newCargoLockSha256 is not --expected-new-cargo-sha/,
    ],
    [
      "a wrong added-record hash",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.added[0].sha256 = "6".repeat(64))),
      }),
      /declared added zfb-js-host@0\.0\.0 bytes hash to [0-9a-f]{64}, not its sha256/,
    ],
    [
      "a wrong changed-record old hash",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.changed[0].oldSha256 = "6".repeat(64))),
      }),
      /declared changed zfb-render@0\.0\.0 old bytes hash to [0-9a-f]{64}, not its oldSha256/,
    ],
    [
      "a wrong changed-record new hash",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.changed[0].newSha256 = "6".repeat(64))),
      }),
      /declared changed zfb-render@0\.0\.0 new bytes hash to [0-9a-f]{64}, not its newSha256/,
    ],
    [
      "a wrong removed-edge delta",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.changed[0].removedDependencies = ["anyhow"])),
      }),
      /removed edges \["sha2"\] differ from removedDependencies/,
    ],
    [
      "a wrong added-edge delta",
      (i) => ({
        ...i,
        declaration: rewireDeclaration(
          (d) => (d.changed[0].addedDependencies = ["zfb-js-host", "zudo-wind"]),
        ),
      }),
      /added edges \["zfb-js-host"\] differ from addedDependencies/,
    ],
    [
      "wrong added-record dependencies",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.added[0].dependencies = ["anyhow"])),
      }),
      /declared added zfb-js-host@0\.0\.0 dependencies differ from the record's/,
    ],
    [
      "a hidden extra record change",
      withNewLock(newText.replace('checksum = "a23e', 'checksum = "b23e')),
      /undeclared changed records anyhow@1\.0\.100/,
    ],
    [
      "an external version move",
      withNewLock(external(newText)),
      /removed records sha2@0\.10\.9; undeclared added records sha2@0\.10\.10/,
    ],
    [
      "an undeclared internal change",
      withNewLock(
        newText.replace(
          'name = "zudo-wind"\nversion = "0.0.0"\ndependencies = [\n "sha2",\n]',
          'name = "zudo-wind"\nversion = "0.0.0"\ndependencies = [\n "anyhow",\n]',
        ),
      ),
      /undeclared changed records zudo-wind@0\.0\.0/,
    ],
    [
      "a removed record",
      withNewLock(
        newText
          .replace(
            '\n[[package]]\nname = "zudo-wind"\nversion = "0.0.0"\ndependencies = [\n "sha2",\n]\n',
            "",
          )
          .replace(/\n\n$/, "\n"),
      ),
      /removed records zudo-wind@0\.0\.0/,
    ],
    [
      "a changed record that moved a non-dependency field",
      withNewLock(
        newText.replace(
          'name = "zfb-render"\nversion = "0.0.0"\n',
          'name = "zfb-render"\nversion = "0.0.0"\nsource = "git+https://example.invalid/zfb#0"\n',
        ),
        (d) =>
          (d.changed[0].newSha256 = rewireSha(
            recordOf(
              newText.replace(
                'name = "zfb-render"\nversion = "0.0.0"\n',
                'name = "zfb-render"\nversion = "0.0.0"\nsource = "git+https://example.invalid/zfb#0"\n',
              ),
              "zfb-render@0.0.0",
            ).body,
          )),
      ),
      /moved a field other than dependencies; declared changed zfb-render@0\.0\.0 has a source or checksum; not an internal record/,
    ],
    [
      "an added record carrying a checksum",
      withNewLock(
        newText.replace(
          'name = "zfb-js-host"\nversion = "0.0.0"\n',
          'name = "zfb-js-host"\nversion = "0.0.0"\nchecksum = "0000"\n',
        ),
        (d) =>
          (d.added[0].sha256 = rewireSha(
            recordOf(
              newText.replace(
                'name = "zfb-js-host"\nversion = "0.0.0"\n',
                'name = "zfb-js-host"\nversion = "0.0.0"\nchecksum = "0000"\n',
              ),
              "zfb-js-host@0.0.0",
            ).body,
          )),
      ),
      /declared added zfb-js-host@0\.0\.0 has a source or checksum; not an internal record/,
    ],
    [
      "an added edge to an undeclared internal record",
      withNewLock(
        newText.replace(
          ' "anyhow",\n "zfb-js-host",\n]',
          ' "anyhow",\n "zfb-js-host",\n "zudo-wind",\n]',
        ),
        (d) => {
          d.changed[0].newSha256 = rewireSha(
            recordOf(
              newText.replace(
                ' "anyhow",\n "zfb-js-host",\n]',
                ' "anyhow",\n "zfb-js-host",\n "zudo-wind",\n]',
              ),
              "zfb-render@0.0.0",
            ).body,
          );
          d.changed[0].addedDependencies = ["zfb-js-host", "zudo-wind"];
        },
      ),
      /changed zfb-render@0\.0\.0 adds an edge to zudo-wind@0\.0\.0, which is not a declared internal record/,
    ],
    [
      "an added edge that resolves to no record",
      withNewLock(
        newText.replace(
          ' "anyhow",\n "sha2",\n]\n\n[[package]]\nname = "zfb-render"',
          ' "anyhow",\n "sha3",\n]\n\n[[package]]\nname = "zfb-render"',
        ),
        (d) => {
          d.added[0].sha256 = rewireSha(
            recordOf(
              newText.replace(
                ' "anyhow",\n "sha2",\n]\n\n[[package]]\nname = "zfb-render"',
                ' "anyhow",\n "sha3",\n]\n\n[[package]]\nname = "zfb-render"',
              ),
              "zfb-js-host@0.0.0",
            ).body,
          );
          d.added[0].dependencies = ["anyhow", "sha3"];
        },
      ),
      /added zfb-js-host@0\.0\.0: dependency entry "sha3" names no record in the new Cargo\.lock/,
    ],
    [
      "a duplicated identity",
      withNewLock(
        newText +
          '\n[[package]]\nname = "zudo-wind"\nversion = "0.0.0"\ndependencies = [\n "anyhow",\n]\n',
      ),
      /duplicate package identity zudo-wind@0\.0\.0 in the new Cargo\.lock/,
    ],
    [
      "a header change",
      withNewLock(newText.replace("version = 4", "version = 3")),
      /the header differs/,
    ],
    [
      "a manifest without publish = false",
      (i) => ({
        ...i,
        readManifest: manifests((path, text) =>
          path.endsWith("zfb-render/Cargo.toml") ? text.replace("publish = false\n", "") : text,
        ),
      }),
      /zfb-render@0\.0\.0: crates\/zfb-render\/Cargo\.toml does not set publish = false/,
    ],
    [
      "a manifest naming another package",
      (i) => ({
        ...i,
        readManifest: manifests((path, text) =>
          path.endsWith("zfb-js-host/Cargo.toml")
            ? text.replace('"zfb-js-host"', '"zfb-js-host2"')
            : text,
        ),
      }),
      /crates\/zfb-js-host\/Cargo\.toml is zfb-js-host2@0\.0\.0, not zfb-js-host@0\.0\.0/,
    ],
    [
      "a manifest at another version",
      (i) => ({
        ...i,
        readManifest: manifests((path, text) =>
          path.endsWith("zfb-js-host/Cargo.toml")
            ? text.replace('version = "0.0.0"', 'version = "0.1.0"')
            : text,
        ),
      }),
      /is zfb-js-host@0\.1\.0, not zfb-js-host@0\.0\.0/,
    ],
    [
      "a missing manifest",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.added[0].manifest = "crates/other/Cargo.toml")),
      }),
      /zfb-js-host@0\.0\.0: cannot read crates\/other\/Cargo\.toml/,
    ],
    [
      "a manifest outside the workspace members",
      (i) => ({
        ...i,
        readManifest: manifests((path, text) =>
          path === "Cargo.toml" ? '[workspace]\nmembers = ["packages/*"]\n' : text,
        ),
      }),
      /crates\/zfb-js-host\/Cargo\.toml is not a workspace member path/,
    ],
    [
      "a root manifest without members",
      (i) => ({
        ...i,
        readManifest: manifests((path, text) => (path === "Cargo.toml" ? "[workspace]\n" : text)),
      }),
      /the root Cargo\.toml lists no \[workspace\] members/,
    ],
    [
      "a wrong-target closure receipt",
      (i) => ({ ...i, closures: closures(DARWIN_TARGET) }),
      /old closure receipt was recorded for target aarch64-apple-darwin, not x86_64-unknown-linux-gnu/,
    ],
    [
      "a closure receipt from another lock",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, { old: (r) => (r.cargoLockSha256 = "7".repeat(64)) }),
      }),
      /old closure receipt was recorded at Cargo\.lock 7{64}, not/,
    ],
    [
      "a closure receipt for another producer",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, { new: (r) => (r.producer = "zudo-wind") }),
      }),
      /new closure receipt was recorded for producer zudo-wind, not zfb/,
    ],
    [
      "a closure receipt of another schema",
      (i) => ({ ...i, closures: closures(LINUX_TARGET, { new: (r) => (r.schemaVersion = 2) }) }),
      /new closure receipt is not a schemaVersion 1 receipt/,
    ],
    [
      "a closure receipt without the producer row",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          new: (r) => (r.rows = r.rows.filter((row) => row.name !== "zfb")),
        }),
      }),
      /new closure receipt does not contain the producer zfb/,
    ],
    [
      "an external closure row with another source",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          new: (r) =>
            (r.rows.find((row) => row.name === "sha2").source = "git+https://example.invalid/sha2"),
        }),
      }),
      /new closure receipt row sha2@0\.10\.9 \(git\+[^)]+\) source differs from its Cargo\.lock record's registry\+/,
    ],
    [
      "an internal closure row claiming a registry source",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          old: (r) =>
            (r.rows.find((row) => row.name === "zudo-wind").source =
              "registry+https://github.com/rust-lang/crates.io-index"),
        }),
      }),
      /old closure receipt row zudo-wind@0\.0\.0 \(registry\+[^)]+\) claims a source, but its Cargo\.lock record is a path record/,
    ],
    [
      "a closure row that is not in its lock",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          new: (r) => r.rows.push({ name: "rand", version: "0.9.0", source: null, features: [] }),
        }),
      }),
      /row rand@0\.9\.0 is not a record of its Cargo\.lock/,
    ],
    [
      "a closure receipt with a repeated row",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, { old: (r) => r.rows.push({ ...r.rows[0] }) }),
      }),
      /old closure receipt repeats row anyhow@1\.0\.100/,
    ],
    [
      "a missing closure receipt",
      (i) => ({ ...i, closures: { old: i.closures.old } }),
      /new closure receipt is missing or has no sha256/,
    ],
    [
      "external feature drift",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          new: (r) =>
            (r.rows.find((row) => row.name === "sha2").features = ["default", "std", "asm"]),
        }),
      }),
      /external feature drift in the producer closure: sha2@0\.10\.9 \(registry\+[^)]+\) \["default","std"\] -> \["asm","default","std"\]/,
    ],
    [
      "an external row joining the closure",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          old: (r) => (r.rows = r.rows.filter((row) => row.name !== "anyhow")),
        }),
      }),
      /external rows joined the producer closure: anyhow@1\.0\.100 \(registry\+/,
    ],
    [
      "a row leaving the closure",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          new: (r) => (r.rows = r.rows.filter((row) => row.name !== "zudo-wind")),
        }),
      }),
      /rows left the producer closure: zudo-wind@0\.0\.0/,
    ],
    [
      "an internal feature move the declaration did not expect",
      (i) => ({
        ...i,
        closures: closures(LINUX_TARGET, {
          new: (r) => (r.rows.find((row) => row.name === "zfb-render").features = ["default"]),
        }),
      }),
      /closure rows changed \["zfb-render@0\.0\.0"\] differ from the declaration's closure\.changed/,
    ],
    [
      "a declaration expecting a closure row that did not appear",
      (i) => ({
        ...i,
        declaration: rewireDeclaration((d) => (d.closure.changed = ["zfb-render@0.0.0"])),
      }),
      /closure rows changed \[\] differ from the declaration's closure\.changed/,
    ],
  ])("refuses %s", (_name, edit, message) => {
    expect(() => prove(edit)).toThrow(message);
  });

  it("records a digest-only new-lock reference outside a checkout and cites a matching ref", () => {
    const dir = mkdtempSync(join(tmpdir(), "zfb-rewire-nogit-"));
    roots.push(dir);
    const notices = [];
    expect(newLockReference(dir, Buffer.from(newText), undefined, (m) => notices.push(m))).toEqual({
      sha256: NEW,
    });
    expect(notices.join("\n")).toContain("not at HEAD; recorded digest only");
  });

  // End-to-end CLI runs against one git-backed checkout.
  describe("end to end", () => {
    const gitRoots = [];
    const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
    const TEST = "scripts/__tests__/island-size-budget.test.mjs";
    const README = "research/v3-island-size/README.md";
    const OUTPUTS = [CONTRACT, TEST, README];
    const SOURCE_SHA = "b".repeat(40);
    const HEAD_SHA = "c".repeat(40);
    let repo;
    let artifact;
    let pinned;
    let rewireInputs;
    let saved;
    let newLockCommit;
    const at = (relative) => join(repo.root, relative);
    const read = (relative) => readFileSync(at(relative));
    const contractText = (value) => JSON.stringify(value, null, 2) + "\n";
    const tempDir = (prefix) => {
      const dir = mkdtempSync(join(tmpdir(), prefix));
      gitRoots.push(dir);
      return dir;
    };
    const linuxArtifact = (edit, tamper) => {
      const dir = tempDir("zfb-rewire-artifact-");
      writeRuns(dir, Object.keys(liveContract.ceilings.workspace), (mode) => {
        const provenance = {
          sourceSha: SOURCE_SHA,
          mode,
          platform: { os: "linux", arch: "x64" },
          fixtureVersion: liveContract.fixtureVersion,
          fixtureSha256: repo.fixtureSha256,
          runnerSha256: repo.runnerSha256,
          packageVersion: repo.version,
          runtimePackageVersion: repo.version,
          honoPackageVersion: liveContract.toolchain.honoVersion,
          esbuildVersion: liveContract.toolchain.esbuildVersion,
          nodeVersion: liveContract.toolchain.nodeVersion,
          zlibVersion: liveContract.toolchain.zlibVersion,
          compression: liveContract.compression,
          repeats: 2,
          pnpmLockSha256: repo.pnpmLockSha256,
          dependencyFingerprint: {
            policyVersion: repo.dependencyFingerprint.policyVersion,
            sha256: repo.dependencyFingerprint.sha256,
          },
          cargoLockSha256: NEW,
        };
        edit?.(provenance, mode);
        return provenance;
      });
      tamper?.(dir);
      return dir;
    };

    beforeAll(() => {
      repo = makeGitCheckout(gitRoots, {
        oldCargoLock: oldText,
        newCargoLock: newText,
        prepare: (checkout) => writeRewireManifests(checkout.root),
      });
      repo.git("tag", "old-lock");
      artifact = linuxArtifact();
      pinned = withCurrentNormalizer(structuredClone(preRewireContract));
      pinned.ceilings = ceilingsOf(artifact);
      pinned.fixtureSha256 = { ...repo.fixtureSha256 };
      pinned.runnerSha256 = { ...repo.runnerSha256 };
      pinned.toolchain.packageVersion = repo.version;
      pinned.toolchain.pnpmLockSha256 = repo.pnpmLockSha256;
      pinned.toolchain.cargoLockSha256 = OLD;
      pinned.dependencyFingerprint = { ...repo.dependencyFingerprint };
      writeFileSync(at(CONTRACT), contractText(pinned));
      rewireInputs = writeRewireInputs(at("research/v3-island-size/rewire"), LINUX_TARGET);
      // HEAD now stores the new lock, the pinned contract and the inputs.
      repo.git("add", "-A");
      repo.git("commit", "-qm", "test fixture: new Cargo.lock, pinned contract, rewire inputs");
      newLockCommit = repo.git("rev-parse", "HEAD");
      saved = Object.fromEntries([...OUTPUTS, "Cargo.lock"].map((file) => [file, read(file)]));
    });
    afterEach(() => {
      // A refusal fixture may commit invalid manifests; restore the temporary
      // repository's cited HEAD as well as its working bytes for the next test.
      repo.git("reset", "--mixed", newLockCommit);
      for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
      writeRewireManifests(repo.root);
      vi.restoreAllMocks();
    });
    afterAll(() => {
      for (const dir of gitRoots.splice(0)) rmSync(dir, { recursive: true, force: true });
    });

    const argv = (extra = {}) =>
      Object.entries({
        artifact,
        "run-id": "81",
        "job-id": "82",
        "head-sha": HEAD_SHA,
        "source-sha": SOURCE_SHA,
        reason: "cargo-internal-rewire",
        "expected-old-cargo-sha": OLD,
        "expected-new-cargo-sha": NEW,
        "old-cargo-ref": "old-lock",
        ...rewireInputs,
        stage: join(tempDir("zfb-rewire-stage-"), "stage"),
        ...extra,
      })
        .filter(([, value]) => value !== undefined)
        .flatMap(([key, value]) => ["--" + key, value]);
    const quiet = () => {
      vi.spyOn(console, "log").mockImplementation(() => {});
      return vi.spyOn(console, "error").mockImplementation(() => {});
    };
    const unchanged = (before) =>
      OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));
    const live = () => OUTPUTS.map((file) => saved[file]);

    it("stages the rewire with exact-key movement, cited sources and a bound run identity", async () => {
      const notices = quiet();
      const stage = join(tempDir("zfb-rewire-stage-"), "stage");
      await main(argv({ stage }), repo.root);
      unchanged(live());
      expect(notices).not.toHaveBeenCalled();
      const next = JSON.parse(readFileSync(join(stage, CONTRACT), "utf8"));
      const moved = contractDiffKeys(pinned, next);
      expect(moved.filter((key) => !key.startsWith("baselineEvidence."))).toEqual([
        "baselineSourceSha",
        "outcome",
        "platformStatus",
        "toolchain.cargoLockSha256",
      ]);
      expect(() =>
        assertAllowedMovement(
          "cargo-internal-rewire",
          CARGO_INTERNAL_REWIRE_MOVEMENT,
          pinned,
          next,
        ),
      ).not.toThrow();
      expect(next.toolchain).toEqual({ ...pinned.toolchain, cargoLockSha256: NEW });
      expect(next.ceilings).toEqual(pinned.ceilings);
      expect(next.dependencyFingerprint).toEqual(pinned.dependencyFingerprint);
      expect(next.outcome).toBe("cargo-internal-rewire-transition");
      expect(next.baselineSourceSha).toBe(HEAD_SHA);
      expect(next.baselineEvidence).toMatchObject({
        event: "ci",
        runId: 81,
        jobId: 82,
        sourceSha: SOURCE_SHA,
      });
      expect(Object.keys(next.baselineEvidence)).toEqual([
        "event",
        "runId",
        "jobId",
        "sourceSha",
        "repinnedFrom",
        "cargoRewire",
        "actualGzip",
        "note",
      ]);
      expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
      expect(next.baselineEvidence.repinnedFrom.contract).toEqual({
        sha256: rewireSha(saved[CONTRACT]),
        commit: newLockCommit,
        path: CONTRACT,
      });
      const rewire = next.baselineEvidence.cargoRewire;
      expect(rewire.oldLockSource).toEqual({
        commit: repo.commit,
        path: "Cargo.lock",
        sha256: OLD,
      });
      expect(rewire.newLockSource).toEqual({
        sha256: NEW,
        commit: newLockCommit,
        path: "Cargo.lock",
      });
      expect(rewire.declaration).toEqual({
        path: "research/v3-island-size/rewire/declaration.json",
        sha256: rewireSha(readFileSync(rewireInputs["rewire-declaration"])),
      });
      expect(rewire.closure).toMatchObject({
        target: LINUX_TARGET,
        oldSha256: rewireSha(readFileSync(rewireInputs["old-closure"])),
        newSha256: rewireSha(readFileSync(rewireInputs["new-closure"])),
        rows: { old: 5, new: 6, unchanged: 5 },
      });
      expect(rewire.added.map((r) => r.id)).toEqual(["zfb-js-host@0.0.0"]);
      expect(rewire.manifestsAtCommit).toBe(newLockCommit);
      expect(rewire.changed[0].manifestSha256).toBe(
        rewireSha(REWIRE_MANIFESTS["crates/zfb-render/Cargo.toml"]),
      );
      expect(rewire.changed[0]).toMatchObject({
        id: "zfb-render@0.0.0",
        removedDependencies: ["sha2"],
        addedDependencies: ["zfb-js-host"],
      });
      expect(next.platformStatus).toContain("after a reviewed internal Cargo.lock rewire");
      expect(next.platformStatus).toContain(
        "PR merge source " + SOURCE_SHA + " (probe head " + HEAD_SHA + ")",
      );
      expect(
        next.baselineEvidence.note.startsWith("Cargo internal-rewire transition run at v"),
      ).toBe(true);
      expect(next.baselineEvidence.note).toContain(
        "Reviewed internal Cargo.lock rewire at unchanged v",
      );
      const readme = readFileSync(join(stage, README), "utf8");
      expect(readme.startsWith(saved[README].toString("utf8"))).toBe(true);
      expect(readme).toContain("## v" + repo.version + " cargo-internal-rewire transition");
      expect(readme).toContain("`" + OLD + "` to `" + NEW + "`");
      expect(readme).toContain("[workflow run 81]");
      expect(readme).toContain("`zfb-render@0.0.0` removes `sha2`, adds `zfb-js-host`");
      expect(readme).toContain("Darwin arm64 contract moves in the same paired change");
      expect(readFileSync(join(stage, TEST), "utf8")).toBe(
        rewriteTestPins(saved[TEST].toString("utf8"), next),
      );
      const manifest = JSON.parse(readFileSync(join(stage, "stage-manifest.json"), "utf8"));
      expect(Object.keys(manifest.destinations)).toEqual([...OUTPUTS].sort());
      expect(manifest.runs).toEqual([
        {
          reason: "cargo-internal-rewire",
          platform: "linux-x64",
          sourceSha: SOURCE_SHA,
          headSha: HEAD_SHA,
          runId: 81,
          artifactId: null,
          previousContract: {
            sha256: rewireSha(saved[CONTRACT]),
            commit: newLockCommit,
            path: CONTRACT,
          },
          transition: {
            from: {
              packageVersion: pinned.toolchain.packageVersion,
              cargoLockSha256: OLD,
              dependencyFingerprint: pinned.dependencyFingerprint,
            },
            to: {
              packageVersion: pinned.toolchain.packageVersion,
              cargoLockSha256: NEW,
              pnpmLockSha256: repo.pnpmLockSha256,
              dependencyFingerprint: pinned.dependencyFingerprint,
            },
          },
          rewire: {
            declarationSha256: rewire.declaration.sha256,
            oldLockSha256: OLD,
            newLockSha256: NEW,
          },
        },
      ]);
      // Linux alone cannot land: the paired Darwin run is missing.
      expect(() => applyStage(stage, repo.root, () => {})).toThrow(
        /--reason cargo-internal-rewire is not staged for darwin-arm64; stage both platforms into the same directory first \(nothing was applied\)/,
      );
      unchanged(live());
    });

    it("refuses dirty manifests when the new lock automatically cites HEAD", async () => {
      quiet();
      const before = OUTPUTS.map(read);
      const stage = join(tempDir("zfb-rewire-stage-"), "stage");
      writeRewireManifests(repo.root, (path, text) =>
        path.endsWith("zfb-render/Cargo.toml") ? text + "# uncommitted manifest edit\n" : text,
      );
      await expect(main(argv({ stage }), repo.root)).rejects.toThrow(
        /crates\/zfb-render\/Cargo\.toml in the working tree differs from [0-9a-f]{40}:crates\/zfb-render\/Cargo\.toml; refusing/,
      );
      expect(existsSync(stage)).toBe(false);
      unchanged(before);
    });

    it("cites --new-cargo-ref only when byte-identical and corroborates --old-cargo-lock", async () => {
      quiet();
      const stage = join(tempDir("zfb-rewire-stage-"), "stage");
      await main(argv({ stage, "new-cargo-ref": "HEAD" }), repo.root);
      const cited = JSON.parse(readFileSync(join(stage, CONTRACT), "utf8")).baselineEvidence
        .cargoRewire;
      expect(cited.newLockSource).toEqual({
        sha256: NEW,
        commit: newLockCommit,
        path: "Cargo.lock",
      });
      expect(cited.manifestsAtCommit).toBe(newLockCommit);
      expect(cited.added[0].manifestSha256).toBe(
        rewireSha(REWIRE_MANIFESTS["crates/zfb-js-host/Cargo.toml"]),
      );
      const before = OUTPUTS.map(read);
      // An uncommitted manifest edit is refused once the manifests are bound to the cited commit.
      writeRewireManifests(repo.root, (path, text) =>
        path.endsWith("zfb-render/Cargo.toml") ? text + "# edited after the commit\n" : text,
      );
      await expect(main(argv({ "new-cargo-ref": "HEAD" }), repo.root)).rejects.toThrow(
        /crates\/zfb-render\/Cargo\.toml in the working tree differs from [0-9a-f]{40}:crates\/zfb-render\/Cargo\.toml; refusing/,
      );
      writeRewireManifests(repo.root);
      await expect(main(argv({ "new-cargo-ref": "old-lock" }), repo.root)).rejects.toThrow(
        /--new-cargo-ref old-lock: [0-9a-f]{40}:Cargo\.lock is not byte-identical to the checkout's Cargo\.lock; refusing/,
      );
      writeFileSync(at("old.lock"), oldText);
      const corroborated = join(tempDir("zfb-rewire-stage-"), "stage");
      await main(argv({ stage: corroborated, "old-cargo-lock": at("old.lock") }), repo.root);
      expect(readdirSync(corroborated)).toContain("stage-manifest.json");
      writeFileSync(at("old.lock"), oldText + "\n");
      await expect(main(argv({ "old-cargo-lock": at("old.lock") }), repo.root)).rejects.toThrow(
        /--old-cargo-lock differs from [0-9a-f]{40}:Cargo\.lock/,
      );
      rmSync(at("old.lock"), { force: true });
      unchanged(before);
    });

    // Each refusal leaves the contract, the budget test and the README unchanged
    // and the stage directory empty.
    const sidecar = (mode, edit) => (dir) => {
      const path = join(dir, "real-" + mode, "measurement.normalized.json");
      const value = JSON.parse(readFileSync(path, "utf8"));
      edit(value);
      writeFileSync(path, JSON.stringify(value));
    };
    const identity = (name, edit) => ({
      name: "a changed " + name,
      contract: edit,
      message: new RegExp(
        "--reason cargo-internal-rewire: allows only Cargo\\.lock to move, but " +
          name.replace(".", "\\.") +
          " differ from the contract",
      ),
    });
    it.each([
      {
        name: "a wrong --expected-old-cargo-sha",
        args: { "expected-old-cargo-sha": "1".repeat(64) },
        message: /the contract pins Cargo\.lock [0-9a-f]{64}, not --expected-old-cargo-sha/,
      },
      {
        name: "a wrong --expected-new-cargo-sha",
        args: { "expected-new-cargo-sha": "2".repeat(64) },
        message: /the checkout's Cargo\.lock hashes to [0-9a-f]{64}, not --expected-new-cargo-sha/,
      },
      {
        name: "an unresolvable --old-cargo-ref",
        args: { "old-cargo-ref": "no-such-ref" },
        message: /--old-cargo-ref no-such-ref does not resolve to exactly one commit/,
      },
      {
        name: "an option-like --old-cargo-ref",
        args: { "old-cargo-ref": "-x" },
        message: /--old-cargo-ref must name a git ref/,
      },
      {
        name: "an --old-cargo-ref whose lock is not the pinned one",
        args: { "old-cargo-ref": "HEAD" },
        message: /[0-9a-f]{40}:Cargo\.lock hashes to [0-9a-f]{64}, not --expected-old-cargo-sha/,
      },
      {
        name: "a declaration with a wrong added-record hash",
        inputs: { declaration: (d) => (d.added[0].sha256 = "6".repeat(64)) },
        message: /declared added zfb-js-host@0\.0\.0 bytes hash to/,
      },
      {
        name: "a declaration for another producer",
        inputs: { declaration: (d) => (d.producer = "zudo-wind") },
        message: /producer must be zfb, the measured CLI package, not "zudo-wind"/,
      },
      {
        name: "a closure row whose source is not the lock's",
        inputs: {
          newClosure: (r) =>
            (r.rows.find((row) => row.name === "anyhow").source =
              "git+https://example.invalid/anyhow"),
        },
        message: /source differs from its Cargo\.lock record's/,
      },
      {
        name: "a declaration with an undeclared edge",
        inputs: { declaration: (d) => (d.changed[0].addedDependencies = []) },
        message: /added edges \["zfb-js-host"\] differ from addedDependencies/,
      },
      {
        name: "a declaration that is not JSON",
        setup: () => writeFileSync(rewireInputs["rewire-declaration"], "{"),
        message: /--rewire-declaration .* is not JSON/,
      },
      {
        name: "a Darwin closure receipt",
        inputs: { target: DARWIN_TARGET },
        message:
          /old closure receipt was recorded for target aarch64-apple-darwin, not x86_64-unknown-linux-gnu/,
      },
      {
        name: "a closure receipt from another lock",
        inputs: { newClosure: (r) => (r.cargoLockSha256 = "7".repeat(64)) },
        message: /new closure receipt was recorded at Cargo\.lock 7{64}/,
      },
      {
        name: "external feature drift",
        inputs: {
          newClosure: (r) => (r.rows.find((row) => row.name === "anyhow").features = ["std"]),
        },
        message: /external feature drift in the producer closure: anyhow@1\.0\.100/,
      },
      {
        name: "a manifest without publish = false",
        setup: () => {
          writeRewireManifests(repo.root, (path, text) =>
            path.endsWith("zfb-js-host/Cargo.toml")
              ? text.replace("publish = false\n", "publish = true\n")
              : text,
          );
          repo.git("commit", "-qam", "fixture: committed publishable internal crate");
        },
        message: /crates\/zfb-js-host\/Cargo\.toml does not set publish = false/,
      },
      {
        name: "publish metadata hidden in a multiline description",
        setup: () => {
          writeRewireManifests(repo.root, (path, text) =>
            path.endsWith("zfb-js-host/Cargo.toml")
              ? text.replace("publish = false\n", 'description = """\npublish = false\n"""\n')
              : text,
          );
          repo.git("commit", "-qam", "fixture: committed multiline description");
        },
        message: /unsupported multiline TOML strings/,
      },
      {
        name: "workspace membership mentioned only in a comment",
        setup: () => {
          writeRewireManifests(repo.root, (path, text) =>
            path === "Cargo.toml"
              ? '[workspace]\nmembers = [\n"crates/zfb-render", # "crates/zfb-js-host"\n]\nresolver = "2"\n'
              : text,
          );
          repo.git("commit", "-qam", "fixture: commented workspace member");
        },
        message: /crates\/zfb-js-host\/Cargo\.toml is not a workspace member path/,
      },
      {
        name: "a wrong-platform artifact",
        provenance: (p) => (p.platform = { os: "darwin", arch: "arm64" }),
        message: /not linux-x64; Darwin arm64 uses rebaseline-island-size-darwin\.mjs/,
      },
      {
        name: "an artifact measured at the old lock",
        provenance: (p) => (p.cargoLockSha256 = OLD),
        message: /artifact Cargo\.lock hash differs from this checkout's Cargo\.lock/,
      },
      {
        name: "a --source-sha that is not the artifact's",
        args: { "source-sha": "d".repeat(40) },
        message: /workspace artifact measured source b{40}, not --source-sha/,
      },
      {
        name: "raw +1",
        contract: (c) => (c.ceilings.workspace.model.raw -= 1),
        message:
          /raw totals changed \(workspace\/model\); this is not a cargo-internal-rewire re-pin/,
      },
      {
        name: "raw -1",
        contract: (c) => (c.ceilings.packed.model.raw += 1),
        message: /raw totals changed \(packed\/model\)/,
      },
      {
        name: "gzipNormalized +1",
        contract: (c) => (c.ceilings.workspace["show-for"].gzipNormalized -= 1),
        message: /gzipNormalized totals changed \(workspace\/show-for \+1\)/,
      },
      {
        name: "gzipNormalized -1",
        contract: (c) => (c.ceilings.packed["json-api"].gzipNormalized += 1),
        message: /gzipNormalized totals changed \(packed\/json-api -1\)/,
      },
      {
        name: "a changed fixture set",
        contract: (c) => (c.ceilings.workspace.extra = { raw: 1, gzipNormalized: 1 }),
        message: /fixture set differs from the contract \(workspace\/extra\)/,
      },
      {
        name: "a moved dependency fingerprint",
        contract: (c) => (c.dependencyFingerprint.sha256 = "8".repeat(64)),
        message:
          /--reason cargo-internal-rewire cannot move the dependency fingerprint \(sha256 differ/,
      },
      identity("packageVersion", (c) => (c.toolchain.packageVersion = "0.0.1")),
      identity("pnpmLockSha256", (c) => (c.toolchain.pnpmLockSha256 = "f".repeat(64))),
      identity("normalizerSha256", (c) => (c.normalizerSha256 = "e".repeat(64))),
      identity("fixtureSha256", (c) => (c.fixtureSha256.model = "0".repeat(64))),
      identity("runnerSha256", (c) => (c.runnerSha256["measure.mjs"] = "0".repeat(64))),
      ...["honoVersion", "esbuildVersion", "nodeVersion", "zlibVersion"].map((key) =>
        identity("toolchain." + key, (c) => (c.toolchain[key] = "0.0.0-test")),
      ),
      {
        name: "a schemaVersion 2 contract",
        contract: (c) => {
          delete c.dependencyFingerprint;
          c.schemaVersion = 2;
        },
        message:
          /--reason cargo-internal-rewire needs a schemaVersion 3 contract, found schemaVersion 2/,
      },
      {
        name: "a contract pinning the new lock already",
        contract: (c) => (c.toolchain.cargoLockSha256 = NEW),
        message: /the contract pins Cargo\.lock [0-9a-f]{64}, not --expected-old-cargo-sha/,
      },
      {
        name: "a sidecar whose results were edited",
        tamper: sidecar("packed", (s) => (s.results.model.shipped.gzipNormalized -= 1)),
        message:
          /real-packed: measurement\.normalized\.json results disagree with the recomputation from the saved bytes/,
      },
      {
        name: "a sidecar whose source.sha256 is stale",
        tamper: sidecar("workspace", (s) => (s.source.sha256 = "0".repeat(64))),
        message: /stale sidecar/,
      },
      {
        name: "a missing measurement.json",
        tamper: (dir) => rmSync(join(dir, "real-packed", "measurement.json")),
        message: "artifact is missing real-packed/measurement.json",
      },
      {
        name: "a missing fixture directory",
        tamper: (dir) => rmSync(join(dir, "real-workspace", "model"), { recursive: true }),
        message: /saved case directories differ from measurement\.json results/,
      },
      {
        name: "a cargo-producer-lock-only flag",
        args: { "added-packages": "zfb-js-host@0.0.0" },
        message: "--added-packages is only for --reason cargo-producer-lock",
      },
      {
        name: "a reviewed external transition flag",
        args: { "reviewed-cargo-transition": "external-transition.json" },
        message: "--reviewed-cargo-transition is only for --reason cargo-producer-lock",
      },
      {
        name: "a missing test-pin anchor",
        setup: () =>
          writeFileSync(
            at(TEST),
            saved[TEST].toString("utf8").replace("records the reviewed Linux x64 ceilings", "x"),
          ),
        message: /Linux ceiling test not found/,
      },
    ])(
      "refuses $name end to end and writes nothing",
      async ({ contract: edit, provenance, tamper, args, inputs: edits, setup, message }) => {
        quiet();
        if (edit) {
          const contract = structuredClone(pinned);
          edit(contract);
          writeFileSync(at(CONTRACT), contractText(contract));
        }
        let values = {};
        if (edits) {
          const { target = LINUX_TARGET, ...rest } = edits;
          values = writeRewireInputs(tempDir("zfb-rewire-inputs-"), target, rest);
        }
        setup?.();
        try {
          const dir = provenance || tamper ? linuxArtifact(provenance, tamper) : artifact;
          const before = OUTPUTS.map(read);
          const stage = join(tempDir("zfb-rewire-stage-"), "stage");
          await expect(
            main(argv({ artifact: dir, stage, ...values, ...args }), repo.root),
          ).rejects.toThrow(message);
          unchanged(before);
          expect(existsSync(stage)).toBe(false);
        } finally {
          writeFileSync(
            rewireInputs["rewire-declaration"],
            JSON.stringify(rewireDeclaration(), null, 2) + "\n",
          );
        }
      },
    );

    it.each([
      [
        "a missing --stage",
        { stage: undefined },
        "--reason cargo-internal-rewire re-pins both platforms together and only stages",
      ],
      ["a missing --source-sha", { "source-sha": undefined }, "required --source-sha"],
      [
        "a missing --rewire-declaration",
        { "rewire-declaration": undefined },
        "required --rewire-declaration",
      ],
      ["a missing --old-closure", { "old-closure": undefined }, "required --old-closure"],
      ["a missing --new-closure", { "new-closure": undefined }, "required --new-closure"],
      ["a missing --old-cargo-ref", { "old-cargo-ref": undefined }, "required --old-cargo-ref"],
      [
        "a short --expected-new-cargo-sha",
        { "expected-new-cargo-sha": "abc" },
        "--expected-new-cargo-sha must be a 64-character SHA-256",
      ],
      [
        "a .. --new-cargo-ref path",
        { "new-cargo-ref": "HEAD:../Cargo.lock" },
        "--new-cargo-ref path must not contain a .. segment",
      ],
    ])("refuses %s in parseArgs, before reading anything", async (_name, extra, message) => {
      expect(() => parseArgs(argv(extra))).toThrow(message);
      const before = OUTPUTS.map(read);
      await expect(main(argv(extra), repo.root)).rejects.toThrow(message);
      unchanged(before);
    });

    it("keeps the rewire arguments to this reason and the shared Cargo arguments to the two lock reasons", () => {
      const base = [
        "--artifact",
        "dir",
        "--run-id",
        "1",
        "--job-id",
        "2",
        "--head-sha",
        "c".repeat(40),
      ];
      for (const key of REWIRE_ARGS) {
        expect(() => parseArgs([...base, "--reason", "release-bump", "--" + key, "x"])).toThrow(
          "--" + key + " is only for --reason cargo-internal-rewire",
        );
        expect(() =>
          parseArgs([...base, "--reason", "cargo-producer-lock", "--" + key, "x"]),
        ).toThrow("--" + key + " is only for --reason cargo-internal-rewire");
      }
      expect(() =>
        parseArgs([...base, "--reason", "release-bump", "--old-cargo-ref", "x"]),
      ).toThrow("--old-cargo-ref is only for --reason cargo-producer-lock, cargo-internal-rewire");
    });

    it("refuses a stage that failed mid-write and a stage directory that mixes reasons", async () => {
      quiet();
      const stage = join(tempDir("zfb-rewire-partial-"), "stage");
      let writes = 0;
      const io = {
        mkdirSync,
        writeFileSync(path, bytes) {
          if (++writes === 2) throw new Error("injected stage write failure");
          writeFileSync(path, bytes);
        },
      };
      await expect(main(argv({ stage }), repo.root, io)).rejects.toThrow(
        /staging failed \(injected stage write failure\); the stage directory may now hold a partial stage/,
      );
      unchanged(live());
      await expect(main(["--apply", stage], repo.root)).rejects.toThrow(/no stage-manifest\.json/);
      await expect(main(argv({ stage }), repo.root)).rejects.toThrow(
        /not empty and has no stage-manifest\.json; refusing/,
      );
      rmSync(stage, { recursive: true, force: true });
      await main(argv({ stage }), repo.root);
      const manifestPath = join(stage, "stage-manifest.json");
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
      manifest.runs.push({
        ...manifest.runs[0],
        reason: "dependency-graph",
        platform: "darwin-arm64",
      });
      writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
      await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
        "the stage directory mixes --reason cargo-internal-rewire, dependency-graph; stage one reason per directory (nothing was applied)",
      );
      await expect(main(argv({ stage }), repo.root)).rejects.toThrow(
        /already staged|mixes --reason/,
      );
      unchanged(live());
    });
  });
});
