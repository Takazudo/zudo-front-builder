#!/usr/bin/env node
// Darwin arm64 island-size contract metric migration (#4169, source #4158)
// and schema-2 provenance re-pin (#4170).
//
// `--reason release-bump` / `--reason runtime-identity` re-pin a
// schemaVersion-2 contract to a new package version / lock hashes / normalizer
// digest taken from a native Darwin arm64 artifact (the hard-coded Darwin pins
// in scripts/__tests__/island-size-budget.test.mjs are rewritten alongside),
// with the same policy as
// rebaseline-island-size-linux.mjs: every raw AND gzipNormalized total must be
// exactly equal to the current contract (zero allowance, increase or decrease),
// so only provenance fields move. A changed total needs a reviewed decision.
// Runner digests (runnerSha256) are NOT re-pinnable here: the validator requires
// the artifact's to equal the contract's, so a runner change needs its own
// reviewed procedure.
//
// `--reason metric-migration` converts the schemaVersion-1 actual-gzip Darwin contract
// (research/v3-island-size/decision.json) to the schemaVersion-2
// `gzip-identity-normalized` shape from a retained native Darwin arm64 CI
// artifact. Deliberately a separate, narrower entry point than
// rebaseline-island-size-linux.mjs: it supports only `metric-migration`, only
// the Darwin arm64 contract, and takes every provenance value from the
// artifact's own saved files instead of the current checkout, because the
// artifact is the evidence (the caller passes the artifact's source SHA).
//
// It re-derives all totals from the saved pass trees, requires the
// normalize-identity.mjs sidecars to exist and agree with that recomputation,
// and refuses any changed raw total, fixture-set difference, platform,
// provenance or saved-file inconsistency. It cannot approve growth: every
// migrated gzipNormalized ceiling must not exceed the previous actual-gzip
// ceiling it replaces, and a re-pin cannot change any total at all.
//
// `--reason fingerprint-migration` (#4187) converts the schemaVersion-2 contract
// to schemaVersion 3, which pins the dependency-graph fingerprint of
// pnpm-lock.yaml (research/v3-island-size/lock-fingerprint.mjs) instead of the
// whole-file hash. The artifact must describe this checkout byte-for-byte
// (lockfiles, version, fixtures, runners and fingerprint); the runner digests
// may differ from the old contract only because the runners now record the
// fingerprint. Totals, package version and normalizer must be unchanged. The
// artifact's own check-budget run against the schemaVersion-2 contract is
// expected to fail (a v2 contract rejects a run carrying the fingerprint); it is
// recorded as evidence, not required to pass. `release-bump` and
// `runtime-identity` accept a schemaVersion-2 or -3 contract and refuse any
// fingerprint movement on v3. The Linux-only `lockfile-provenance` is rejected.
//
// `--reason fingerprint-helper --base-ref <ref>` (#4198) takes no artifact: it
// runs the Linux script's shared runFingerprintHelperRepin, which re-pins only
// `dependencyFingerprint.helperSha256` of both contracts in one run after
// proving the pinned and new lock-fingerprint.mjs agree on the real lock.
//
// `--reason cargo-producer-lock` (#4214, schemaVersion 3) records one reviewed
// Cargo.lock move from a fresh native artifact, additive by default, through the Linux
// script's shared proveCargoProducerLock: old and new hashes are bound, every
// old `[[package]]` record must survive byte-identical, and the additions must
// equal the `--added-packages` allowlist. Only the Cargo hash and its evidence
// move. `release-bump` and `runtime-identity` refuse any Cargo.lock movement.
// The old lock comes only from git: `--old-cargo-ref <ref>[:<path>]` is resolved
// once in the supplied checkout and recorded as `oldLockSource: {commit, path,
// sha256}`; `--old-cargo-lock <file>` is optional corroboration (#4219).
//
// `release-bump`, `runtime-identity` and `cargo-producer-lock` record the flat
// immediate predecessor `baselineEvidence.repinnedFrom` through the Linux
// script's predecessorRecord, with a `contract` reference to the previous
// contract bytes (SHA-256, plus commit and path when HEAD or an explicit
// `--previous-contract-ref` stores exactly those bytes). `baselineSourceSha` is
// the measured source; `contract.commit` is where the previous contract is
// stored.
//
// `--reason dependency-graph` (#4218, schemaVersion 3) re-pins only
// `dependencyFingerprint.sha256` (and the recorded lock hash and evidence) from
// a fresh native artifact measured at a checkout already carrying the new
// graph, through the Linux script's shared refusals and exact-key movement
// check, and regenerates lock-fingerprint-inventory.json from `--explain`. The
// saved sidecars must agree with the saved bytes (loadDarwinArtifact's
// verifySidecar). `--stage <dir>` and `--apply <dir>` use the Linux script's
// shared staging, so both platforms stage into one directory and are applied
// together; without `--stage` the reason is refused (checkPairedReasonStage).
// Each run records its transition (stageRunIdentity); both must record one
// transition, and `--source-sha` must be the Linux run's measured merge source
// or its probe head, or staging and `--apply` refuse.
//
// `--reason fingerprint-policy` (#4218, schemaVersion 3) moves the four
// `dependencyFingerprint` keys only together with an exact one-step
// `policyVersion` increment that equals the checkout helper's exported
// FINGERPRINT_POLICY_VERSION (the Linux script's shared
// fingerprintPolicyRefusal), refuses everything dependency-graph refuses, writes
// only FINGERPRINT_POLICY_MOVEMENT, regenerates the inventory and rewrites the
// policy identity line of LOCK-FINGERPRINT-POLICY.md. It is staged and applied
// in pairs like dependency-graph. The checkout's own lock-fingerprint.mjs
// (loadCheckoutHelper) computes every fingerprint.
//
// `--reason cargo-internal-rewire` (#4258, schemaVersion 3) records one
// reviewed internal Cargo.lock rewire (an added unpublished workspace crate
// and/or rewired dependency edges of an existing unpublished workspace record)
// through the Linux script's shared proveCargoInternalRewire: the declared
// transition (`--rewire-declaration`), the old lock blob at `--old-cargo-ref`,
// the checkout's new lock, the referenced workspace manifests and the two
// `aarch64-apple-darwin` producer closure receipts (`--old-closure`,
// `--new-closure`) must all agree, every other record must be byte-identical
// and no external package/feature may drift. It is paired and source-bound
// like dependency-graph: `--stage` is required and `--apply` lands both
// platforms only when their transitions and declaration digests agree.
//
// No file is written before every validation and every rendered output
// succeeds; there is no force or ignore-provenance option.
// Optional --reviewed-cargo-transition uses the same exact non-additive #318
// proof as Linux. Native Darwin artifact checks and equal-total gates remain.

import {
  LOCAL_EVIDENCE_PROFILES,
  assertLocalSecurityTransition,
  localEvidenceProfile,
  readLocalEvidence,
  verifyLocalCheckout,
  verifyLocalStageEvidence,
} from "./island-size-local-evidence.mjs";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  IDENTITY_NORMALIZATION_VERSION,
  IDENTITY_NORMALIZED_METRIC,
  NORMALIZED_SIDECAR,
  loadMeasurementDirectory,
  normalizeMeasurement,
  normalizerSha256,
} from "../research/v3-island-size/normalize-identity.mjs";
import {
  CARGO_INTERNAL_REWIRE_REASON,
  CARGO_PRODUCER_LOCK_REASON,
  DEPENDENCY_GRAPH_MOVEMENT,
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_HELPER_REASON,
  FINGERPRINT_MOVE_REMEDY,
  FINGERPRINT_POLICY_MOVEMENT,
  FINGERPRINT_POLICY_REASON,
  INVENTORY_REASONS,
  POLICY_DOC,
  REASONS,
  SOURCE_BOUND_REASONS,
  acceptedSchemas,
  applyStage,
  assertAllowedMovement,
  assertCargoInternalRewireMovement,
  assertCargoProducerLockMovement,
  cargoMoveRefusal,
  cargoRewireMarkdown,
  cargoRewireText,
  cargoTransitionMarkdown,
  cargoTransitionText,
  checkApplyArgs,
  checkCargoInternalRewireArgs,
  checkCargoProducerLockArgs,
  checkFingerprintHelperArgs,
  checkPairedReasonStage,
  checkPreviousContractRefArgs,
  artifactFingerprintProblem,
  contractFingerprint,
  dependencyGraphFingerprintRefusal,
  dependencyGraphIdentityMoves,
  dependencyGraphIdentityRefusal,
  describeDeltas,
  diffCeilings,
  fingerprintTransitionMarkdown,
  fingerprintTransitionRecord,
  fingerprintTransitionText,
  fingerprintPolicyRefusal,
  fingerprintPolicyTransitionMarkdown,
  fingerprintPolicyTransitionRecord,
  fingerprintPolicyTransitionText,
  loadCheckoutHelper,
  measuredTotals,
  openOutputSession,
  previousContractReference,
  proveCargoInternalRewire,
  proveCargoProducerLock,
  readCargoLockEvidence,
  readCargoRewireEvidence,
  readCheckoutIdentity,
  regenerateInventory,
  repinnedFromRecord,
  rewritePolicyIdentity,
  rewriteTestBlock,
  runFingerprintHelperRepin,
  savedSidecarProblem,
  stageRunIdentity,
} from "./rebaseline-island-size-linux.mjs";

const MODES = ["workspace", "packed"];
const CONTRACT = "research/v3-island-size/decision.json";
const README = "research/v3-island-size/README.md";
const TEST_FILE = "scripts/__tests__/island-size-budget.test.mjs";
const INVENTORY_FILE = "research/v3-island-size/lock-fingerprint-inventory.json";
const TEST_ANCHOR = 'it("records the reviewed Darwin arm64 ceilings';
const PLATFORM = { os: "darwin", arch: "arm64" };
const METRIC_MIGRATION = "metric-migration";
const REPIN_REASONS = ["release-bump", "runtime-identity"];
const FINGERPRINT_MIGRATION = "fingerprint-migration";
const DARWIN_REASONS = [
  METRIC_MIGRATION,
  ...REPIN_REASONS,
  FINGERPRINT_MIGRATION,
  FINGERPRINT_HELPER_REASON,
  CARGO_PRODUCER_LOCK_REASON,
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_POLICY_REASON,
  CARGO_INTERNAL_REWIRE_REASON,
];
// The Darwin reasons that record `repinnedFrom` and accept --previous-contract-ref.
const DARWIN_REPINNED_FROM_REASONS = [
  ...REPIN_REASONS,
  CARGO_PRODUCER_LOCK_REASON,
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_POLICY_REASON,
  CARGO_INTERNAL_REWIRE_REASON,
];
// The producer closure target a Darwin arm64 rewire receipt must be recorded for.
export const DARWIN_PRODUCER_TARGET = "aarch64-apple-darwin";
const shaPattern = /^[0-9a-f]{64}$/;
const versionPattern = /^\d+\.\d+\.\d+(?:-next\.\d+)?$/;

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function samePlatform(platform) {
  return platform?.os === PLATFORM.os && platform?.arch === PLATFORM.arch;
}

function sameJson(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function parseRunnerProvenance(text) {
  const values = {};
  for (const line of text.split("\n")) {
    const at = line.indexOf("=");
    if (at > 0) values[line.slice(0, at)] = line.slice(at + 1);
  }
  return values;
}

// `sha256sum`-style lines: "<64 hex>  <path>".
function leadingDigest(value) {
  return typeof value === "string" ? value.split(/\s+/)[0] : undefined;
}

// Loads everything the migration trusts from the artifact directory; every
// number the contract will carry is recomputed from the saved bytes here.
// `verifySidecar` refuses here, before any validation, a saved sidecar that
// is stale or disagrees with the recomputation (savedSidecarProblem, shared
// with the Linux script); validateDarwinEvidence reports the same for every
// reason.
export function loadDarwinArtifact(artifactDir, { verifySidecar = false } = {}) {
  const dir = resolve(artifactDir);
  const modes = {};
  for (const mode of MODES) {
    const modeDir = join(dir, "real-" + mode);
    if (!existsSync(join(modeDir, "measurement.json"))) {
      throw new Error("artifact is missing real-" + mode + "/measurement.json");
    }
    const sidecarPath = join(modeDir, NORMALIZED_SIDECAR);
    if (!existsSync(sidecarPath)) {
      throw new Error(
        "missing " +
          NORMALIZED_SIDECAR +
          " for " +
          mode +
          "; run normalize-identity.mjs --measurement " +
          join(modeDir, "measurement.json") +
          " over the retained artifact first",
      );
    }
    const { measurementText, artifacts } = loadMeasurementDirectory(modeDir);
    const recomputed = normalizeMeasurement({
      measurementText,
      artifacts,
      normalizerDigest: normalizerSha256(),
    });
    if (verifySidecar) {
      const problem = savedSidecarProblem(modeDir, recomputed);
      if (problem) throw new Error("real-" + mode + ": " + problem + "; refusing");
    }
    modes[mode] = {
      measurement: JSON.parse(measurementText),
      sidecar: JSON.parse(readFileSync(sidecarPath, "utf8")),
      recomputed,
    };
  }
  const runnerPath = join(dir, "runner-provenance.txt");
  if (!existsSync(runnerPath)) throw new Error("artifact is missing runner-provenance.txt");
  const digestOf = (relative) => {
    const path = join(dir, relative);
    return existsSync(path) ? sha256(readFileSync(path)) : undefined;
  };
  const version = modes.packed.measurement.provenance?.packageVersion;
  return {
    modes,
    runner: parseRunnerProvenance(readFileSync(runnerPath, "utf8")),
    files: {
      zfbTarballSha256: digestOf("pack/takazudo-zfb-" + version + ".tgz"),
      runtimeTarballSha256: digestOf("pack/takazudo-zfb-runtime-" + version + ".tgz"),
      slugifyTarballSha256: digestOf("pack/takazudo-zfb-slugify-" + version + ".tgz"),
      packedPackageJsonSha256: digestOf("packed-package/package/package.json"),
      packedRuntimePackageJsonSha256: digestOf("packed-runtime/package/package.json"),
      packedSlugifyPackageJsonSha256: digestOf("packed-slugify/package/package.json"),
      checkerLogSha256: digestOf("check-budget.log"),
      checkerExitCode: existsSync(join(dir, "check-budget.exit-code"))
        ? readFileSync(join(dir, "check-budget.exit-code"), "utf8").trim()
        : undefined,
    },
  };
}

// Returns every reason the artifact cannot back this migration, or [].
export function validateDarwinEvidence(
  contract,
  artifact,
  expectedSourceSha,
  reason = METRIC_MIGRATION,
) {
  const errors = [];
  if (!isRecord(contract) || !samePlatform(contract.platform)) {
    errors.push("contract is not the Darwin arm64 contract");
  }
  const requires = REASONS[reason]?.requires;
  const schemas = REASONS[reason] ? acceptedSchemas(reason) : [];
  if (reason === METRIC_MIGRATION) {
    if (contract?.schemaVersion !== requires || Object.hasOwn(contract ?? {}, "metric")) {
      errors.push("contract is not a schemaVersion " + requires + " actual-gzip contract");
    }
  } else if (
    !schemas.includes(contract?.schemaVersion) ||
    contract?.metric !== IDENTITY_NORMALIZED_METRIC ||
    contract?.normalizationVersion !== IDENTITY_NORMALIZATION_VERSION
  ) {
    errors.push(
      "contract is not a schemaVersion " +
        schemas.join(" or ") +
        " " +
        IDENTITY_NORMALIZED_METRIC +
        " v" +
        IDENTITY_NORMALIZATION_VERSION +
        " contract",
    );
  }
  const reference = artifact.modes.workspace.measurement.provenance ?? {};
  for (const mode of MODES) {
    const { measurement, sidecar, recomputed } = artifact.modes[mode];
    const p = measurement.provenance ?? {};
    const label = mode + ": ";
    if (!samePlatform(p.platform)) errors.push(label + "measured platform is not darwin-arm64");
    if (p.mode !== mode) errors.push(label + "wrong measurement mode");
    if (p.sourceSha !== expectedSourceSha) {
      errors.push(label + "source SHA " + String(p.sourceSha) + " is not " + expectedSourceSha);
    }
    if (p.fixtureVersion !== contract?.fixtureVersion)
      errors.push(label + "fixture version differs");
    if (!sameJson(sortedEntries(p.fixtureSha256), sortedEntries(contract?.fixtureSha256))) {
      errors.push(label + "fixture hashes differ from the contract");
    }
    // The migration's runners gained the fingerprint, so their digests are
    // checked against the checkout (checkoutMismatches), not the old contract.
    if (
      reason !== FINGERPRINT_MIGRATION &&
      !sameJson(sortedEntries(p.runnerSha256), sortedEntries(contract?.runnerSha256))
    ) {
      errors.push(label + "runner hashes differ from the contract");
    }
    if (reason === FINGERPRINT_MIGRATION || contract?.schemaVersion === 3) {
      if (!isRecord(p.dependencyFingerprint)) {
        errors.push(label + "provenance carries no dependencyFingerprint");
      }
    }
    if (!sameJson(p.dependencyFingerprint, reference.dependencyFingerprint)) {
      errors.push(label + "dependencyFingerprint differs between workspace and packed");
    }
    if (!sameJson(sortedEntries(p.runnerSha256), sortedEntries(reference.runnerSha256))) {
      errors.push(label + "runnerSha256 differs between workspace and packed");
    }
    for (const [key, expected] of [
      ["nodeVersion", contract.toolchain?.nodeVersion],
      ["zlibVersion", contract.toolchain?.zlibVersion],
      ["honoPackageVersion", contract.toolchain?.honoVersion],
      ["esbuildVersion", contract.toolchain?.esbuildVersion],
      ["compression", contract.compression],
    ]) {
      if (p[key] !== expected) errors.push(label + key + " differs from the contract");
    }
    if (p.repeats !== 2) errors.push(label + "expected two saved passes");
    if (
      !versionPattern.test(p.packageVersion ?? "") ||
      p.runtimePackageVersion !== p.packageVersion
    ) {
      errors.push(label + "package versions are missing or disagree");
    }
    for (const key of [
      "packageJsonSha256",
      "pnpmLockSha256",
      "cargoLockSha256",
      "zfbBinarySha256",
      "esbuildBinarySha256",
      "zfbTarballSha256",
      "runtimeTarballSha256",
    ]) {
      if (!shaPattern.test(p[key] ?? "")) errors.push(label + "invalid " + key);
    }
    for (const key of [
      "sourceSha",
      "packageVersion",
      "pnpmLockSha256",
      "cargoLockSha256",
      "zfbBinarySha256",
      "esbuildBinarySha256",
      "zfbTarballSha256",
      "runtimeTarballSha256",
    ]) {
      if (p[key] !== reference[key])
        errors.push(label + key + " differs between workspace and packed");
    }
    if (!samePlatform(sidecar?.source?.platform)) {
      errors.push(label + "sidecar was not produced from a darwin-arm64 measurement");
    }
    if (sidecar?.normalizer?.sha256 !== normalizerSha256()) {
      errors.push(label + "sidecar normalizer hash differs from the current normalizer");
    }
    if (!sameJson(withoutNormalizerRuntime(sidecar), withoutNormalizerRuntime(recomputed))) {
      errors.push(label + "sidecar disagrees with the recomputation from the saved bytes");
    }
  }
  const packed = artifact.modes.packed.measurement.provenance ?? {};
  for (const [claimed, actual, name] of [
    [packed.zfbTarballSha256, artifact.files.zfbTarballSha256, "zfb tarball"],
    [packed.runtimeTarballSha256, artifact.files.runtimeTarballSha256, "runtime tarball"],
    [packed.packageJsonSha256, artifact.files.packedPackageJsonSha256, "packed zfb package.json"],
    [
      packed.runtimePackageJsonSha256,
      artifact.files.packedRuntimePackageJsonSha256,
      "packed runtime package.json",
    ],
    [
      packed.slugifyPackageJsonSha256,
      artifact.files.packedSlugifyPackageJsonSha256,
      "packed slugify package.json",
    ],
  ]) {
    if (actual === undefined || claimed !== actual) {
      errors.push("saved " + name + " is missing or differs from the provenance digest");
    }
  }
  const r = artifact.runner;
  const runnerChecks = [
    ["runner", r.runner, "Darwin arm64"],
    ["source_sha", r.source_sha, expectedSourceSha],
    ["node", r.node, contract.toolchain?.nodeVersion],
    ["zlib", r.zlib, contract.toolchain?.zlibVersion],
    ["esbuild_version", r.esbuild_version, contract.toolchain?.esbuildVersion],
    ["zfb_binary", leadingDigest(r.zfb_binary), packed.zfbBinarySha256],
    ["esbuild_binary", leadingDigest(r.esbuild_binary), packed.esbuildBinarySha256],
    ["zfb_tarball", leadingDigest(r.zfb_tarball), packed.zfbTarballSha256],
    ["runtime_tarball", leadingDigest(r.runtime_tarball), packed.runtimeTarballSha256],
    ["slugify_tarball", leadingDigest(r.slugify_tarball), artifact.files.slugifyTarballSha256],
  ];
  for (const [name, found, expected] of runnerChecks) {
    if (found === undefined || found !== expected) {
      errors.push("runner-provenance.txt " + name + " is missing or inconsistent");
    }
  }
  return errors;
}

function sortedEntries(map) {
  return isRecord(map) ? Object.entries(map).sort(([a], [b]) => (a < b ? -1 : 1)) : null;
}

// The normalizer block records where the sidecar was written, not what the
// saved bytes say; every other field must agree with the recomputation.
function withoutNormalizerRuntime(sidecar) {
  if (!isRecord(sidecar)) return sidecar;
  const { normalizer, ...rest } = sidecar;
  return { ...rest, normalizerFile: normalizer?.file };
}

// Guards the migration math: identical raw, identical fixture set, and a
// normalized ceiling that never exceeds the actual-gzip ceiling it replaces.
export function checkMigrationTotals(contract, totals) {
  const errors = [];
  const { missingFixtures, rawChanges } = diffCeilings(contract, totals);
  if (missingFixtures.length > 0) {
    errors.push("fixture set differs from the contract (" + missingFixtures.join(", ") + ")");
  }
  if (rawChanges.length > 0) {
    errors.push(
      "raw totals changed (" +
        rawChanges.map((c) => c.mode + "/" + c.fixture).join(", ") +
        "); a metric migration cannot absorb product size changes",
    );
  }
  for (const mode of MODES) {
    for (const [fixture, measured] of Object.entries(totals[mode])) {
      const before = contract.ceilings?.[mode]?.[fixture];
      const where = mode + "/" + fixture;
      if (measured.gzipNormalized > measured.gzip) {
        errors.push(where + ": gzipNormalized exceeds actual gzip");
      }
      if ((measured.raw === 0) !== (measured.gzipNormalized === 0)) {
        errors.push(where + ": raw and gzipNormalized disagree about an empty dist");
      }
      if (before && measured.gzipNormalized > before.gzip) {
        errors.push(
          where +
            ": gzipNormalized " +
            measured.gzipNormalized +
            " exceeds the previous gzip ceiling " +
            before.gzip +
            "; that is a size increase, not a metric migration",
        );
      }
    }
  }
  return errors;
}

export function migrateDarwinContract(contract, totals, evidence) {
  const errors = checkMigrationTotals(contract, totals);
  if (errors.length > 0) throw new Error(errors.join("; "));
  const previous = {
    schemaVersion: contract.schemaVersion,
    metric: "gzip",
    packageVersion: contract.toolchain.packageVersion,
    baselineSourceSha: contract.baselineSourceSha,
    baselineRunId: contract.baselineEvidence?.runId,
  };
  const next = {};
  for (const [key, value] of Object.entries(structuredClone(contract))) {
    next[key] = value;
    if (key === "schemaVersion") {
      next.schemaVersion = 2;
      next.metric = IDENTITY_NORMALIZED_METRIC;
      next.normalizationVersion = IDENTITY_NORMALIZATION_VERSION;
      next.normalizerSha256 = evidence.normalizerSha256;
    }
  }
  next.outcome = "metric-migration";
  next.baselineSourceSha = evidence.sourceSha;
  next.allowance = { raw: 0, gzipNormalized: 0 };
  next.toolchain.packageVersion = evidence.packageVersion;
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  next.toolchain.cargoLockSha256 = evidence.cargoLockSha256;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    ", replayed by normalize-identity.mjs over the retained native artifact of workflow_dispatch run " +
    evidence.runId +
    " at source " +
    evidence.sourceSha +
    ". All 16 raw totals are unchanged from the v" +
    previous.packageVersion +
    " actual-gzip contract; the gzipNormalized ceilings are new values computed from the saved " +
    "bytes and are not comparable with any earlier gzip ceiling. This is historical-artifact " +
    "evidence, not a native measurement of the current source or lockfiles; that verification is " +
    "tracked in #4170. Workspace and packed eight-fixture matrices each have two byte-identical " +
    "full-dist inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    event: "ci-artifact-replay",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    artifact: {
      id: evidence.artifactId,
      name: "island-size-darwin-arm64-" + evidence.sourceSha,
      expiresAt: evidence.artifactExpiresAt,
    },
    nativeRunner: evidence.nativeRunner,
    migratedFrom: previous,
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      "Metric migration replayed from the retained native Darwin arm64 artifact of run " +
      evidence.runId +
      " (job " +
      evidence.jobId +
      ") at source " +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. The gzipNormalized ceilings were computed by normalize-identity.mjs v" +
      IDENTITY_NORMALIZATION_VERSION +
      " over the saved artifact bytes, not converted arithmetically from the v" +
      previous.packageVersion +
      " actual-gzip ceilings. Raw totals did not change; actual gzip is kept in " +
      "baselineEvidence.actualGzip as transfer-size evidence only. The original checker run " +
      "failed on the old actual-gzip contract by token-only movement and is preserved in the " +
      "artifact. Lock hashes and package version are those of the artifact's source SHA, not " +
      "the current checkout. Ceilings equal the artifact measurements with zero allowance; " +
      "earlier ceilings remain in git history.",
  };
  for (const mode of MODES) {
    for (const fixture of Object.keys(next.ceilings[mode])) {
      next.ceilings[mode][fixture] = {
        raw: totals[mode][fixture].raw,
        gzipNormalized: totals[mode][fixture].gzipNormalized,
      };
    }
  }
  return next;
}

// Zero-allowance guard for a re-pin: every fixture present and both raw and
// gzipNormalized exactly equal to the contract, in either direction.
export function checkRepinTotals(contract, totals) {
  const errors = [];
  const { missingFixtures, rawChanges, normalizedChanges } = diffCeilings(contract, totals);
  if (missingFixtures.length > 0) {
    errors.push("fixture set differs from the contract (" + missingFixtures.join(", ") + ")");
  }
  if (rawChanges.length > 0) {
    errors.push(
      "raw totals changed (" +
        rawChanges.map((c) => c.mode + "/" + c.fixture).join(", ") +
        "); record a reviewed decision instead of a re-pin",
    );
  }
  if (normalizedChanges.length > 0) {
    errors.push(
      "gzipNormalized totals changed (" +
        normalizedChanges.map((c) => c.mode + "/" + c.fixture + " " + signed(c.delta)).join(", ") +
        "); the identity token is normalized away, so this is product code moving — " +
        "record a reviewed decision instead of a re-pin",
    );
  }
  return errors;
}

function signed(n) {
  return n > 0 ? "+" + n : String(n);
}

// Actual gzip movement against the previous contract's recorded transfer-size
// evidence; null when the previous contract carries none.
function gzipMovesSince(contract, totals) {
  const before = contract.baselineEvidence?.actualGzip;
  if (!isRecord(before)) return null;
  const moves = [];
  for (const mode of MODES) {
    for (const [fixture, row] of Object.entries(totals[mode])) {
      const was = before[mode]?.[fixture];
      if (typeof was !== "number") return null;
      if (was !== row.gzip) moves.push({ mode, fixture, delta: row.gzip - was });
    }
  }
  return moves;
}

function gzipSummary(moves) {
  if (moves === null) return "no previous actual gzip evidence to compare";
  if (moves.length === 0) return "no actual gzip total moved";
  return (
    moves.length +
    " actual gzip totals moved (" +
    describeDeltas(moves) +
    "; evidence only, not a ceiling)"
  );
}

// `checkout` carries the checkout's {dependencyFingerprint, dependencyFingerprintError}.
function fingerprintMoves(contract, checkout) {
  return Object.keys(contractFingerprint({})).filter(
    (key) => contract.dependencyFingerprint?.[key] !== checkout?.dependencyFingerprint?.[key],
  );
}

// Provenance-only re-pin of a schemaVersion-2 or -3 contract. Ceilings, metric,
// fixtures, allowance, proposedCiLane and a v3 dependencyFingerprint are
// carried over untouched; a v3 fingerprint movement is refused.
export function repinDarwinContract(contract, totals, evidence, reason, checkout) {
  if (!REPIN_REASONS.includes(reason)) throw new Error("unknown re-pin --reason " + reason);
  const repinnedFrom = repinnedFromRecord(contract, evidence.previousContract);
  const errors = checkRepinTotals(contract, totals);
  const cargo = cargoMoveRefusal(reason, evidence.cargoLockSha256, contract);
  if (cargo) errors.push(cargo);
  if (contract.schemaVersion === 3) {
    const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout ?? {});
    if (problem) errors.push(problem);
    const moved = fingerprintMoves(contract, checkout);
    if (!problem && moved.length > 0) {
      errors.push(
        "--reason " +
          reason +
          " cannot move the dependency fingerprint (" +
          moved.join(", ") +
          " differ from the contract); " +
          FINGERPRINT_MOVE_REMEDY,
      );
    }
  }
  if (errors.length > 0) throw new Error(errors.join("; "));
  const policy = REASONS[reason];
  const previousVersion = contract.toolchain.packageVersion;
  const moves = gzipMovesSince(contract, totals);
  const next = structuredClone(contract);
  next.outcome = policy.outcome;
  next.normalizerSha256 = evidence.normalizerSha256;
  next.baselineSourceSha = evidence.sourceSha;
  next.toolchain.packageVersion = evidence.packageVersion;
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    ", re-pinned (" +
    reason +
    ") from the native artifact of workflow_dispatch run " +
    evidence.runId +
    " at source " +
    evidence.sourceSha +
    ". Every raw and gzipNormalized total is unchanged from the v" +
    previousVersion +
    " contract; equal totals alone do not prove byte-identical product code. " +
    gzipSummary(moves) +
    ". Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    event: "ci-artifact",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    artifact: {
      id: evidence.artifactId,
      name: "island-size-darwin-arm64-" + evidence.sourceSha,
      expiresAt: evidence.artifactExpiresAt,
    },
    nativeRunner: evidence.nativeRunner,
    repinnedFrom,
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      "Re-pin (" +
      reason +
      ") from the native Darwin arm64 artifact of run " +
      evidence.runId +
      " (job " +
      evidence.jobId +
      ") at source " +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. Raw and gzipNormalized totals did not change from the v" +
      previousVersion +
      " contract; " +
      gzipSummary(moves) +
      ". Only package version, the pnpm-lock.yaml hash and the normalizer digest were re-pinned; Cargo.lock is unchanged. " +
      "Ceilings equal the artifact measurements with zero allowance; earlier ceilings remain in git history.",
  };
  return { contract: next, gzipMoves: moves ?? [], previousVersion };
}

// schemaVersion 2 -> 3: the whole-file lock pin becomes the dependency-graph
// fingerprint. Totals, package version, normalizer and Cargo.lock must not move;
// the runner digests and the recorded lock hash follow the artifact.
export function migrateDarwinFingerprint(contract, totals, evidence, checkout) {
  const errors = checkRepinTotals(contract, totals);
  if (contract.schemaVersion !== 2) {
    errors.push(
      "--reason " +
        FINGERPRINT_MIGRATION +
        " needs a schemaVersion 2 contract, found schemaVersion " +
        String(contract.schemaVersion),
    );
  }
  const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout ?? {});
  if (problem) errors.push(problem);
  const moved = [];
  if (evidence.packageVersion !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (evidence.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  if (evidence.cargoLockSha256 !== contract.toolchain.cargoLockSha256) {
    moved.push("cargoLockSha256");
  }
  if (moved.length > 0) {
    errors.push(
      "--reason " +
        FINGERPRINT_MIGRATION +
        " changes only the lock pin, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change first",
    );
  }
  if (errors.length > 0) throw new Error(errors.join("; "));
  const fingerprint = contractFingerprint(checkout.dependencyFingerprint);
  const moves = gzipMovesSince(contract, totals);
  const next = {};
  for (const [key, value] of Object.entries(structuredClone(contract))) {
    next[key] = value;
    if (key === "schemaVersion") next.schemaVersion = 3;
    if (key === "normalizerSha256") next.dependencyFingerprint = fingerprint;
  }
  next.outcome = FINGERPRINT_MIGRATION;
  next.baselineSourceSha = evidence.sourceSha;
  next.runnerSha256 = { ...evidence.runnerSha256 };
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  next.toolchain.cargoLockSha256 = evidence.cargoLockSha256;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    ", migrated to schemaVersion 3 from the native artifact of workflow_dispatch run " +
    evidence.runId +
    " at source " +
    evidence.sourceSha +
    ". The contract now pins the dependency-graph fingerprint of pnpm-lock.yaml (policy v" +
    fingerprint.policyVersion +
    ", " +
    fingerprint.parser +
    ") instead of the whole-file hash, which stays recorded as baseline provenance only. The " +
    "package version, fixtures, Cargo.lock, toolchain and normalizer are unchanged; the runner " +
    "digests follow the measured artifact because the runners now record the fingerprint. Every " +
    "raw and gzipNormalized total is unchanged; equal totals alone do not prove byte-identical " +
    "product code. " +
    gzipSummary(moves) +
    ". Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    event: "ci-artifact",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    artifact: {
      id: evidence.artifactId,
      name: "island-size-darwin-arm64-" + evidence.sourceSha,
      expiresAt: evidence.artifactExpiresAt,
    },
    nativeRunner: evidence.nativeRunner,
    migratedFrom: {
      schemaVersion: contract.schemaVersion,
      outcome: contract.outcome,
      baselineSourceSha: contract.baselineSourceSha,
      baselineRunId: contract.baselineEvidence?.runId,
    },
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      "Dependency-fingerprint migration (schemaVersion 2 to 3) from the native Darwin arm64 " +
      "artifact of run " +
      evidence.runId +
      " (job " +
      evidence.jobId +
      ") at source " +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. The fingerprint " +
      fingerprint.sha256 +
      " was computed by lock-fingerprint.mjs on the checkout and equals the artifact's; the " +
      "pnpm-lock.yaml hash is the artifact's own lock, recorded as provenance only. The original " +
      "checker run compared the artifact with the schemaVersion 2 contract, which cannot accept a " +
      "run carrying the fingerprint; its result is preserved in the artifact. Raw and " +
      "gzipNormalized totals did not change from the v" +
      contract.toolchain.packageVersion +
      " contract; " +
      gzipSummary(moves) +
      ". Ceilings equal the artifact measurements with zero allowance; earlier ceilings remain in git history.",
  };
  return {
    contract: next,
    gzipMoves: moves ?? [],
    previousVersion: contract.toolchain.packageVersion,
  };
}

// One additive or approved exact non-additive Cargo.lock move on a schemaVersion-3 contract. The
// package version, pnpm lock, normalizer and fingerprint must equal the
// contract (fixtures, runners and toolchain are checked by
// validateDarwinEvidence); `cargoLock` is readCargoLockEvidence's result.
export function transitionDarwinCargoLock(contract, totals, evidence, checkout, cargoLock) {
  const repinnedFrom = repinnedFromRecord(contract, evidence.previousContract);
  const errors = checkRepinTotals(contract, totals);
  if (contract.schemaVersion !== 3) {
    errors.push(
      "--reason " +
        CARGO_PRODUCER_LOCK_REASON +
        " needs a schemaVersion 3 contract, found schemaVersion " +
        String(contract.schemaVersion),
    );
  }
  const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout ?? {});
  if (problem) errors.push(problem);
  const moved = fingerprintMoves(contract, checkout).map((key) => "dependencyFingerprint." + key);
  if (evidence.packageVersion !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (evidence.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  if (evidence.pnpmLockSha256 !== contract.toolchain.pnpmLockSha256) moved.push("pnpmLockSha256");
  if (!problem && moved.length > 0) {
    errors.push(
      "--reason " +
        CARGO_PRODUCER_LOCK_REASON +
        " allows only Cargo.lock to move, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change",
    );
  }
  if (errors.length > 0) throw new Error(errors.join("; "));
  const transition = proveCargoProducerLock({
    ...cargoLock,
    contractSha: contract.toolchain.cargoLockSha256,
    artifactSha: evidence.cargoLockSha256,
  });
  if (evidence.localReceipt) assertLocalSecurityTransition(transition);
  const moves = gzipMovesSince(contract, totals);
  const next = structuredClone(contract);
  next.outcome = REASONS[CARGO_PRODUCER_LOCK_REASON].outcome;
  next.baselineSourceSha = evidence.sourceSha;
  next.toolchain.cargoLockSha256 = transition.newSha;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    (evidence.localReceipt
      ? ", re-pinned from owner-attested local native evidence " + evidence.localReceipt.sha256
      : ", re-pinned from the native artifact of workflow_dispatch run " + evidence.runId) +
    " at source " +
    evidence.sourceSha +
    " after a reviewed " +
    (transition.review ? "non-additive" : "additive") +
    " Cargo.lock transition (" +
    cargoTransitionText(transition) +
    "). The package version, fixtures, runners, pnpm-lock.yaml and its dependency fingerprint, " +
    "toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; " +
    "only the Cargo.lock hash moved. Equal totals alone do not prove byte-identical product code. " +
    gzipSummary(moves) +
    ". Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    ...(evidence.localReceipt
      ? {
          event: "local-native",
          localReceipt: evidence.localReceipt,
          stagingTool: evidence.stagingTool,
        }
      : {
          event: "ci-artifact",
          runId: evidence.runId,
          jobId: evidence.jobId,
          artifact: {
            id: evidence.artifactId,
            name: "island-size-darwin-arm64-" + evidence.sourceSha,
            expiresAt: evidence.artifactExpiresAt,
          },
        }),
    sourceSha: evidence.sourceSha,
    nativeRunner: evidence.nativeRunner,
    repinnedFrom,
    cargoTransition: transition,
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      (evidence.localReceipt
        ? "Cargo producer-lock transition from owner-attested local native Darwin arm64 evidence at source "
        : "Cargo producer-lock transition from the native Darwin arm64 artifact of run " +
          evidence.runId +
          " (job " +
          evidence.jobId +
          ") at source ") +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. Raw and gzipNormalized totals did not change from the v" +
      contract.toolchain.packageVersion +
      " contract; " +
      gzipSummary(moves) +
      ". The artifact's own check-budget run compared it with the previous Cargo.lock pin and is " +
      "preserved as evidence. Only the Cargo.lock hash was re-pinned. Ceilings equal the artifact " +
      "measurements with zero allowance; earlier ceilings remain in git history.",
  };
  assertCargoProducerLockMovement(contract, next);
  return { contract: next, gzipMoves: moves ?? [] };
}

export function darwinCargoProducerLockReadmeSection(contract, gzipMoves) {
  const e = contract.baselineEvidence;
  return (
    "\n## Darwin v" +
    contract.toolchain.packageVersion +
    " " +
    REASONS[CARGO_PRODUCER_LOCK_REASON].heading +
    "\n\n" +
    "The Darwin arm64 contract recorded a reviewed " +
    (e.cargoTransition.review ? "non-additive" : "additive") +
    " `Cargo.lock` transition at v" +
    contract.toolchain.packageVersion +
    (e.event === "local-native"
      ? " from owner-attested local native evidence (receipt SHA-256 `" +
        e.localReceipt.sha256 +
        "`) at source `"
      : " from the native artifact " +
        e.artifact.id +
        " of [workflow_dispatch run " +
        e.runId +
        "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
        e.runId +
        ") (job " +
        e.jobId +
        ") at source `") +
    e.sourceSha +
    "`: " +
    cargoTransitionMarkdown(e.cargoTransition) +
    " The package version, fixtures, runners, `pnpm-lock.yaml` and its dependency fingerprint, " +
    "toolchain and normalizer are unchanged, and every raw and `gzipNormalized` total is " +
    "unchanged; only `toolchain.cargoLockSha256` moved. These totals alone do not prove " +
    "byte-identical product code. The artifact's own `check-budget.mjs` run against the previous " +
    "pin is preserved as evidence (exit code " +
    String(e.originalCheckerResult?.exitCode) +
    "). " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence in `baselineEvidence.actualGzip`, not a ceiling.") +
    " Linux x64 is re-pinned separately by `rebaseline-island-size-linux.mjs`.\n"
  );
}

// One reviewed internal Cargo.lock rewire on a schemaVersion-3 contract. The
// package version, pnpm lock, normalizer and fingerprint must equal the
// contract (fixtures, runners and toolchain are checked by
// validateDarwinEvidence); `rewire` is readCargoRewireEvidence's result for
// DARWIN_PRODUCER_TARGET, proven by the shared proveCargoInternalRewire.
export function transitionDarwinCargoRewire(contract, totals, evidence, checkout, rewire) {
  const reason = CARGO_INTERNAL_REWIRE_REASON;
  const repinnedFrom = repinnedFromRecord(contract, evidence.previousContract);
  const errors = checkRepinTotals(contract, totals);
  if (contract.schemaVersion !== 3) {
    errors.push(
      "--reason " +
        reason +
        " needs a schemaVersion 3 contract, found schemaVersion " +
        String(contract.schemaVersion),
    );
  }
  const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout ?? {});
  if (problem) errors.push(problem);
  const moved = fingerprintMoves(contract, checkout).map((key) => "dependencyFingerprint." + key);
  if (evidence.packageVersion !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (evidence.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  if (evidence.pnpmLockSha256 !== contract.toolchain.pnpmLockSha256) moved.push("pnpmLockSha256");
  if (!problem && moved.length > 0) {
    errors.push(
      "--reason " +
        reason +
        " allows only Cargo.lock to move, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change",
    );
  }
  if (errors.length > 0) throw new Error(errors.join("; "));
  const transition = proveCargoInternalRewire({
    ...rewire,
    contractSha: contract.toolchain.cargoLockSha256,
    artifactSha: evidence.cargoLockSha256,
  });
  const moves = gzipMovesSince(contract, totals);
  const next = structuredClone(contract);
  next.outcome = REASONS[reason].outcome;
  next.baselineSourceSha = evidence.sourceSha;
  next.toolchain.cargoLockSha256 = transition.newSha;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    (evidence.localReceipt
      ? ", re-pinned from owner-attested local native evidence " + evidence.localReceipt.sha256
      : ", re-pinned from the native artifact of workflow_dispatch run " + evidence.runId) +
    " at source " +
    evidence.sourceSha +
    " after a reviewed internal Cargo.lock rewire (" +
    cargoRewireText(transition) +
    "). The package version, fixtures, runners, pnpm-lock.yaml and its dependency fingerprint, " +
    "toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; " +
    "only the Cargo.lock hash moved. Equal totals alone do not prove byte-identical product code. " +
    gzipSummary(moves) +
    ". Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    ...(evidence.localReceipt
      ? {
          event: "local-native",
          localReceipt: evidence.localReceipt,
          stagingTool: evidence.stagingTool,
        }
      : {
          event: "ci-artifact",
          runId: evidence.runId,
          jobId: evidence.jobId,
          artifact: {
            id: evidence.artifactId,
            name: "island-size-darwin-arm64-" + evidence.sourceSha,
            expiresAt: evidence.artifactExpiresAt,
          },
        }),
    sourceSha: evidence.sourceSha,
    nativeRunner: evidence.nativeRunner,
    repinnedFrom,
    cargoRewire: transition,
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      (evidence.localReceipt
        ? "Cargo internal-rewire transition from owner-attested local native Darwin arm64 evidence at source "
        : "Cargo internal-rewire transition from the native Darwin arm64 artifact of run " +
          evidence.runId +
          " (job " +
          evidence.jobId +
          ") at source ") +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. Raw and gzipNormalized totals did not change from the v" +
      contract.toolchain.packageVersion +
      " contract; " +
      gzipSummary(moves) +
      ". The artifact's own check-budget run compared it with the previous Cargo.lock pin and is " +
      "preserved as evidence. Only the Cargo.lock hash was re-pinned. Ceilings equal the artifact " +
      "measurements with zero allowance; earlier ceilings remain in git history.",
  };
  assertCargoInternalRewireMovement(contract, next);
  return { contract: next, gzipMoves: moves ?? [] };
}

export function darwinCargoRewireReadmeSection(contract, gzipMoves) {
  const e = contract.baselineEvidence;
  return (
    "\n## Darwin v" +
    contract.toolchain.packageVersion +
    " " +
    REASONS[CARGO_INTERNAL_REWIRE_REASON].heading +
    "\n\n" +
    "The Darwin arm64 contract recorded a reviewed internal `Cargo.lock` rewire at v" +
    contract.toolchain.packageVersion +
    (e.event === "local-native"
      ? " from owner-attested local native evidence (receipt SHA-256 `" +
        e.localReceipt.sha256 +
        "`) at source `"
      : " from the native artifact " +
        e.artifact.id +
        " of [workflow_dispatch run " +
        e.runId +
        "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
        e.runId +
        ") (job " +
        e.jobId +
        ") at source `") +
    e.sourceSha +
    "`: " +
    cargoRewireMarkdown(e.cargoRewire) +
    " The package version, fixtures, runners, `pnpm-lock.yaml` and its dependency fingerprint, " +
    "toolchain and normalizer are unchanged, and every raw and `gzipNormalized` total is " +
    "unchanged; only `toolchain.cargoLockSha256` moved. These totals alone do not prove " +
    "byte-identical product code. The artifact's own `check-budget.mjs` run against the previous " +
    "pin is preserved as evidence (exit code " +
    String(e.originalCheckerResult?.exitCode) +
    "). " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence in `baselineEvidence.actualGzip`, not a ceiling.") +
    " Linux x64 moves in the same paired change (`--stage`, then `--apply`).\n"
  );
}

// One reviewed dependency-graph move on a schemaVersion-3 contract: the
// artifact carries this checkout's fingerprint, exactly the contract's
// `dependencyFingerprint.sha256` moves, and the totals, Cargo.lock, package
// version, normalizer, fixtures, runners, metric and allowance are unchanged
// (the toolchain is checked by validateDarwinEvidence). Only
// DEPENDENCY_GRAPH_MOVEMENT is written.
export function repinDarwinDependencyGraph(contract, totals, evidence, checkout) {
  const reason = DEPENDENCY_GRAPH_REASON;
  const repinnedFrom = repinnedFromRecord(contract, evidence.previousContract);
  const totalsErrors = checkRepinTotals(contract, totals);
  if (totalsErrors.length > 0) throw new Error(totalsErrors.join("; "));
  if (contract.schemaVersion !== 3) {
    throw new Error(
      "--reason " +
        reason +
        " needs a schemaVersion 3 contract, found schemaVersion " +
        String(contract.schemaVersion),
    );
  }
  const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout ?? {});
  if (problem) throw new Error("--reason " + reason + ": " + problem);
  const refusal = dependencyGraphFingerprintRefusal(contract, checkout.dependencyFingerprint);
  if (refusal) throw new Error(refusal);
  const cargo = cargoMoveRefusal(reason, evidence.cargoLockSha256, contract);
  if (cargo) throw new Error(cargo);
  const moved = dependencyGraphIdentityMoves(contract, {
    packageVersion: evidence.packageVersion,
    normalizerSha256: evidence.normalizerSha256,
    fixtureSha256: evidence.fixtureSha256,
    runnerSha256: evidence.runnerSha256,
  });
  if (moved.length > 0) throw new Error(dependencyGraphIdentityRefusal(moved));
  const transition = fingerprintTransitionRecord(contract, checkout.dependencyFingerprint);
  const moves = gzipMovesSince(contract, totals);
  const next = structuredClone(contract);
  next.outcome = REASONS[reason].outcome;
  next.baselineSourceSha = evidence.sourceSha;
  next.dependencyFingerprint.sha256 = transition.newSha256;
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    ", re-pinned from the native artifact of workflow_dispatch run " +
    evidence.runId +
    " at source " +
    evidence.sourceSha +
    " after a reviewed dependency-graph change (" +
    fingerprintTransitionText(transition) +
    "), measured on a checkout already carrying the new graph. The package version, fixtures, " +
    "runners, Cargo.lock, toolchain and normalizer are unchanged, and every raw and gzipNormalized " +
    "total is unchanged; only the dependency fingerprint sha256 and the recorded pnpm-lock.yaml " +
    "hash moved; equal totals alone do not prove byte-identical product code. " +
    gzipSummary(moves) +
    ". Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    event: "ci-artifact",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    artifact: {
      id: evidence.artifactId,
      name: "island-size-darwin-arm64-" + evidence.sourceSha,
      expiresAt: evidence.artifactExpiresAt,
    },
    nativeRunner: evidence.nativeRunner,
    repinnedFrom,
    fingerprintTransition: transition,
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      "Dependency-graph re-pin from the native Darwin arm64 artifact of run " +
      evidence.runId +
      " (job " +
      evidence.jobId +
      ") at source " +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. The new fingerprint was computed by lock-fingerprint.mjs on the checkout and " +
      "equals the artifact's; the artifact's own check-budget run compared it with the previous " +
      "fingerprint pin and is preserved as evidence. Raw and gzipNormalized totals did not change " +
      "from the v" +
      contract.toolchain.packageVersion +
      " contract; " +
      gzipSummary(moves) +
      ". Ceilings equal the artifact measurements with zero allowance; earlier ceilings remain in git history.",
  };
  assertAllowedMovement(reason, DEPENDENCY_GRAPH_MOVEMENT, contract, next);
  return { contract: next, gzipMoves: moves ?? [] };
}

export function darwinDependencyGraphReadmeSection(contract, gzipMoves) {
  const e = contract.baselineEvidence;
  return (
    "\n## Darwin v" +
    contract.toolchain.packageVersion +
    " " +
    REASONS[DEPENDENCY_GRAPH_REASON].heading +
    "\n\n" +
    "The Darwin arm64 contract re-pinned `dependencyFingerprint.sha256` at v" +
    contract.toolchain.packageVersion +
    " from the native artifact " +
    e.artifact.id +
    " of [workflow_dispatch run " +
    e.runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    e.runId +
    ") (job " +
    e.jobId +
    ") at source `" +
    e.sourceSha +
    "` after a reviewed dependency-graph change: " +
    fingerprintTransitionMarkdown(e.fingerprintTransition) +
    ". The package version, fixtures, runners, `Cargo.lock`, toolchain and normalizer are " +
    "unchanged, and every raw and `gzipNormalized` total is unchanged; only " +
    "`dependencyFingerprint.sha256` and the recorded `toolchain.pnpmLockSha256` moved. These " +
    "totals alone do not prove byte-identical product code. The artifact's own `check-budget.mjs` " +
    "run against the previous fingerprint pin is preserved as evidence (exit code " +
    String(e.originalCheckerResult?.exitCode) +
    "). " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence in `baselineEvidence.actualGzip`, not a ceiling.") +
    " Linux x64 moves in the same paired change (`--stage`, then `--apply`).\n"
  );
}

// One reviewed fingerprint-policy move on a schemaVersion-3 contract: the
// artifact carries this checkout's fingerprint, the contract's four
// fingerprint keys move only with a one-step policyVersion increment that the
// checkout helper exports (`checkout.checkoutHelper`), and the totals,
// Cargo.lock, package version, normalizer, fixtures, runners, metric and
// allowance are unchanged (the toolchain is checked by validateDarwinEvidence).
// Only FINGERPRINT_POLICY_MOVEMENT is written.
export function repinDarwinFingerprintPolicy(contract, totals, evidence, checkout) {
  const reason = FINGERPRINT_POLICY_REASON;
  const fail = (message) => new Error("--reason " + reason + ": " + message);
  const repinnedFrom = repinnedFromRecord(contract, evidence.previousContract);
  const totalsErrors = checkRepinTotals(contract, totals);
  if (totalsErrors.length > 0) throw new Error(totalsErrors.join("; "));
  if (contract.schemaVersion !== 3) {
    throw new Error(
      "--reason " +
        reason +
        " needs a schemaVersion 3 contract, found schemaVersion " +
        String(contract.schemaVersion),
    );
  }
  const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout ?? {});
  if (problem) throw fail(problem);
  if (!isRecord(checkout.checkoutHelper)) {
    throw fail("needs the checkout helper's exported identity");
  }
  const refusal = fingerprintPolicyRefusal(
    contract,
    checkout.dependencyFingerprint,
    checkout.checkoutHelper,
  );
  if (refusal) throw fail(refusal);
  const cargo = cargoMoveRefusal(reason, evidence.cargoLockSha256, contract);
  if (cargo) throw new Error(cargo);
  const moved = dependencyGraphIdentityMoves(contract, {
    packageVersion: evidence.packageVersion,
    normalizerSha256: evidence.normalizerSha256,
    fixtureSha256: evidence.fixtureSha256,
    runnerSha256: evidence.runnerSha256,
  });
  if (moved.length > 0) throw new Error(dependencyGraphIdentityRefusal(moved, reason));
  const transition = fingerprintPolicyTransitionRecord(contract, checkout.dependencyFingerprint);
  const moves = gzipMovesSince(contract, totals);
  const next = structuredClone(contract);
  next.outcome = REASONS[reason].outcome;
  next.baselineSourceSha = evidence.sourceSha;
  next.dependencyFingerprint = { ...transition.to };
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  const metric = IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
  next.platformStatus =
    "Darwin arm64 has exact zero-allowance v" +
    evidence.packageVersion +
    " ceilings on metric " +
    metric +
    ", re-pinned from the native artifact of workflow_dispatch run " +
    evidence.runId +
    " at source " +
    evidence.sourceSha +
    " after a reviewed fingerprint-policy change (" +
    fingerprintPolicyTransitionText(transition) +
    "), measured on a checkout whose lock-fingerprint.mjs exports the new policy. policyVersion " +
    "and sha256 equal the artifact's run provenance; parser and helperSha256 are proven against " +
    "the checkout helper, not attested by the artifact. The package version, fixtures, runners, " +
    "Cargo.lock, toolchain and normalizer are unchanged, and every raw and gzipNormalized total " +
    "is unchanged; only the dependency fingerprint and the recorded pnpm-lock.yaml hash moved; " +
    "equal totals alone do not prove byte-identical product code. " +
    gzipSummary(moves) +
    ". Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  next.baselineEvidence = {
    event: "ci-artifact",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    artifact: {
      id: evidence.artifactId,
      name: "island-size-darwin-arm64-" + evidence.sourceSha,
      expiresAt: evidence.artifactExpiresAt,
    },
    nativeRunner: evidence.nativeRunner,
    repinnedFrom,
    fingerprintPolicyTransition: transition,
    originalCheckerResult: evidence.originalCheckerResult,
    actualGzip: actualGzipRows(totals),
    note:
      "Fingerprint-policy re-pin from the native Darwin arm64 artifact of run " +
      evidence.runId +
      " (job " +
      evidence.jobId +
      ") at source " +
      evidence.sourceSha +
      ". Complete workspace and packed eight-fixture matrices, each with two identical full-dist " +
      "inventories. policyVersion stepped by exactly one to the checkout helper's exported " +
      "FINGERPRINT_POLICY_VERSION; the new policyVersion and sha256 equal the artifact's, and " +
      "parser and helperSha256 are the checkout helper's own. The artifact's own check-budget " +
      "run compared it with the previous fingerprint pin and is preserved as evidence. Raw and " +
      "gzipNormalized totals did not change from the v" +
      contract.toolchain.packageVersion +
      " contract; " +
      gzipSummary(moves) +
      ". Ceilings equal the artifact measurements with zero allowance; earlier ceilings remain in git history.",
  };
  assertAllowedMovement(reason, FINGERPRINT_POLICY_MOVEMENT, contract, next);
  return { contract: next, gzipMoves: moves ?? [] };
}

export function darwinFingerprintPolicyReadmeSection(contract, gzipMoves) {
  const e = contract.baselineEvidence;
  return (
    "\n## Darwin v" +
    contract.toolchain.packageVersion +
    " " +
    REASONS[FINGERPRINT_POLICY_REASON].heading +
    "\n\n" +
    "The Darwin arm64 contract re-pinned its dependency fingerprint at v" +
    contract.toolchain.packageVersion +
    " from the native artifact " +
    e.artifact.id +
    " of [workflow_dispatch run " +
    e.runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    e.runId +
    ") (job " +
    e.jobId +
    ") at source `" +
    e.sourceSha +
    "` after a reviewed fingerprint-policy change: " +
    fingerprintPolicyTransitionMarkdown(e.fingerprintPolicyTransition) +
    ". `policyVersion` stepped by exactly one to the checkout helper's exported " +
    "`FINGERPRINT_POLICY_VERSION`. The package version, fixtures, runners, `Cargo.lock`, " +
    "toolchain and normalizer are unchanged, and every raw and `gzipNormalized` total is " +
    "unchanged; only the four `dependencyFingerprint` keys and the recorded " +
    "`toolchain.pnpmLockSha256` moved. These totals alone do not prove byte-identical product " +
    "code. The artifact's own `check-budget.mjs` run against the previous fingerprint pin is " +
    "preserved as evidence (exit code " +
    String(e.originalCheckerResult?.exitCode) +
    "). " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence in `baselineEvidence.actualGzip`, not a ceiling.") +
    " Linux x64 moves in the same paired change (`--stage`, then `--apply`).\n"
  );
}

function replacePin(block, pattern, value, label) {
  if (!pattern.test(block)) throw new Error("missing Darwin " + label + " pin in " + TEST_FILE);
  return block.replace(pattern, (_, head) => head + value);
}

// Rewrites the provenance pins of the Darwin ceiling test to match a re-pinned
// contract. Ceilings are untouched: a re-pin cannot move them.
export function rewriteDarwinTestPins(source, contract) {
  const e = contract.baselineEvidence;
  const t = contract.toolchain;
  return rewriteTestBlock(source, TEST_ANCHOR, "Darwin ceiling", (block) =>
    [
      [/(darwinContract\.toolchain\.packageVersion\)\.toBe\(\s*)"[^"]+"/, t.packageVersion],
      [/(darwinContract\.baselineSourceSha\)\.toBe\(\s*)"[^"]+"/, contract.baselineSourceSha],
      [/(darwinContract\.toolchain\.pnpmLockSha256\)\.toBe\(\s*)"[^"]+"/, t.pnpmLockSha256],
      [/(darwinContract\.toolchain\.cargoLockSha256\)\.toBe\(\s*)"[^"]+"/, t.cargoLockSha256],
      // Scaffolded under `schemaVersion === 3`; required once the contract is v3.
      ...(contract.schemaVersion === 3
        ? [
            [
              /(darwinContract\.dependencyFingerprint\.sha256\)\.toBe\(\s*)"[^"]+"/,
              contract.dependencyFingerprint.sha256,
            ],
          ]
        : []),
    ]
      .map(([pattern, value]) => [pattern, JSON.stringify(value)])
      .reduce((text, [pattern, value]) => replacePin(text, pattern, value, pattern.source), block)
      .replace(/(darwinContract\.schemaVersion\)\.toBe\()\d+/, "$1" + contract.schemaVersion)
      .replace(
        /expect\(darwinContract\.baselineEvidence\)\.toMatchObject\(\{[\s\S]*?\n    \}\);/,
        () =>
          e.event === "local-native"
            ? "expect(darwinContract.baselineEvidence).toMatchObject({\n" +
              '      event: "local-native",\n' +
              "      localReceipt: { sha256: " +
              JSON.stringify(e.localReceipt.sha256) +
              " },\n" +
              "    });"
            : "expect(darwinContract.baselineEvidence).toMatchObject({\n" +
              '      event: "ci-artifact",\n' +
              "      runId: " +
              e.runId +
              ",\n" +
              "      jobId: " +
              e.jobId +
              ",\n" +
              "      artifact: { id: " +
              e.artifact.id +
              " },\n" +
              "    });",
      ),
  );
}

export function darwinRepinReadmeSection(contract, gzipMoves, previousVersion, reason) {
  const e = contract.baselineEvidence;
  const version = contract.toolchain.packageVersion;
  return (
    "\n## Darwin v" +
    version +
    " " +
    REASONS[reason].heading +
    "\n\n" +
    "The Darwin arm64 contract was re-pinned (`" +
    reason +
    "`) for v" +
    version +
    " from the native artifact " +
    e.artifact.id +
    " of [workflow_dispatch run " +
    e.runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    e.runId +
    ") (job " +
    e.jobId +
    ") at source `" +
    e.sourceSha +
    "`. Every raw and `gzipNormalized` total is unchanged from the v" +
    previousVersion +
    " contract, so the package version, `pnpm-lock.yaml` hash and normalizer digest were re-pinned (the normalizer digest changes only when the normalizer itself changed; `Cargo.lock` is unchanged); these totals alone do not prove byte-identical product code. " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence in `baselineEvidence.actualGzip`, not a ceiling.") +
    " Linux x64 is unaffected.\n"
  );
}

export function darwinFingerprintReadmeSection(contract, gzipMoves) {
  const e = contract.baselineEvidence;
  const fingerprint = contract.dependencyFingerprint;
  return (
    "\n## Darwin v" +
    contract.toolchain.packageVersion +
    " " +
    REASONS[FINGERPRINT_MIGRATION].heading +
    "\n\n" +
    "The Darwin arm64 contract moved from schemaVersion 2 to schemaVersion 3 at v" +
    contract.toolchain.packageVersion +
    " from the native artifact " +
    e.artifact.id +
    " of [workflow_dispatch run " +
    e.runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    e.runId +
    ") (job " +
    e.jobId +
    ") at source `" +
    e.sourceSha +
    "`. It now pins the dependency-graph fingerprint `" +
    fingerprint.sha256 +
    "` (policy v" +
    fingerprint.policyVersion +
    ", `" +
    fingerprint.parser +
    "`, helper `" +
    fingerprint.helperSha256 +
    "`; see [LOCK-FINGERPRINT-POLICY.md](LOCK-FINGERPRINT-POLICY.md)) instead of the whole-file " +
    "`pnpm-lock.yaml` hash. The artifact's lock hash `" +
    contract.toolchain.pnpmLockSha256 +
    "` stays recorded in `toolchain.pnpmLockSha256` as baseline provenance only. The package " +
    "version, fixtures, `Cargo.lock`, toolchain and normalizer are unchanged, the runner digests " +
    "follow the measured artifact because the runners now record the fingerprint, and every raw " +
    "and `gzipNormalized` total is unchanged; these totals alone do not prove byte-identical " +
    "product code. The artifact's own `check-budget.mjs` run against the schemaVersion 2 contract " +
    "is preserved as evidence (exit code " +
    String(e.originalCheckerResult?.exitCode) +
    "); a schemaVersion 2 contract cannot accept a run carrying the fingerprint. " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence in `baselineEvidence.actualGzip`, not a ceiling.") +
    " Linux x64 is migrated separately by `rebaseline-island-size-linux.mjs`.\n"
  );
}

function actualGzipRows(totals) {
  return Object.fromEntries(
    MODES.map((mode) => [
      mode,
      Object.fromEntries(Object.entries(totals[mode]).map(([name, row]) => [name, row.gzip])),
    ]),
  );
}

export function darwinReadmeSection(contract, previousContract) {
  const e = contract.baselineEvidence;
  const gzipMoves = [];
  for (const mode of MODES) {
    for (const [fixture, gzip] of Object.entries(e.actualGzip[mode])) {
      const before = previousContract.ceilings[mode][fixture].gzip;
      if (gzip !== before) gzipMoves.push({ mode, fixture, delta: gzip - before });
    }
  }
  const version = contract.toolchain.packageVersion;
  return (
    "\n## Darwin v" +
    version +
    " metric migration\n\n" +
    "The Darwin arm64 contract moved from actual gzip (schemaVersion 1, v" +
    previousContract.toolchain.packageVersion +
    ") to `" +
    IDENTITY_NORMALIZED_METRIC +
    " v" +
    IDENTITY_NORMALIZATION_VERSION +
    "` (schemaVersion 2) by replaying `normalize-identity.mjs` over the retained native artifact " +
    e.artifact.id +
    " of [workflow_dispatch run " +
    e.runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    e.runId +
    ") (job " +
    e.jobId +
    ") at source `" +
    e.sourceSha +
    "` (v" +
    version +
    "). Original semantics: each ceiling was the exact actual gzip of the emitted JS, so the " +
    "16-hex `zudo_react_build` token moved it by a few bytes per release (the original run failed " +
    "six cases by +1 to +3 gzip bytes with raw unchanged). Migrated semantics: raw bytes plus " +
    "gzip of the same files with only the compiler-owned token sites replaced by a same-length " +
    "placeholder. Every raw total is unchanged from the previous contract and each " +
    "`gzipNormalized` ceiling is at or below the actual-gzip ceiling it replaces. " +
    (gzipMoves.length === 0
      ? "No actual gzip total moved."
      : "Actual gzip moved on " +
        gzipMoves.length +
        " rows (" +
        describeDeltas(gzipMoves) +
        "); that is transfer-size evidence recorded in `baselineEvidence.actualGzip`, not a ceiling.") +
    " The contract records the artifact's own package version and lock hashes, so this is " +
    "historical-artifact evidence and does not claim a native measurement of the current source; " +
    "[#4170](https://github.com/Takazudo/zudo-front-builder/issues/4170) owns that verification. " +
    "Replay recipe: download the artifact, run `node research/v3-island-size/normalize-identity.mjs " +
    "--measurement <artifact>/real-workspace/measurement.json` and the same for `real-packed`, then " +
    "`node scripts/rebaseline-island-size-darwin.mjs --reason metric-migration ...` " +
    "(see the script header). Linux x64 is unaffected.\n"
  );
}

const REQUIRED = [
  "artifact",
  "run-id",
  "job-id",
  "artifact-id",
  "artifact-expires-at",
  "source-sha",
  "reason",
];

export function parseArgs(argv) {
  const values = {};
  const duplicates = [];
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined)
      throw new Error("bad argument: " + argv[i]);
    if (Object.hasOwn(values, argv[i].slice(2))) duplicates.push(argv[i]);
    values[argv[i].slice(2)] = argv[i + 1];
  }
  const local =
    Object.hasOwn(values, "local-receipt") || Object.hasOwn(values, "local-receipt-sha");
  if (local) {
    if (duplicates.length)
      throw new Error("duplicate local-mode argument: " + duplicates.join(", "));
    if (
      !Object.hasOwn(LOCAL_EVIDENCE_PROFILES, values.reason) ||
      Object.hasOwn(values, "apply") ||
      !values.stage
    )
      throw new Error(
        "local evidence requires staged " + Object.keys(LOCAL_EVIDENCE_PROFILES).join(" or "),
      );
    if (!/^[a-f0-9]{40}$/.test(values["tool-sha"] ?? ""))
      throw new Error("local evidence requires an exact --tool-sha");
    if (!values["local-receipt"] || !/^[a-f0-9]{64}$/.test(values["local-receipt-sha"] ?? ""))
      throw new Error("--local-receipt and --local-receipt-sha are required together");
    if (
      ["run-id", "job-id", "artifact-id", "artifact-expires-at"].some((key) =>
        Object.hasOwn(values, key),
      )
    )
      throw new Error("local evidence forbids Actions metadata");
  }
  if (!local && ["source-root", "tool-sha"].some((key) => Object.hasOwn(values, key)))
    throw new Error("--source-root and --tool-sha require local evidence");
  if (checkApplyArgs(values)) return values;
  if (checkFingerprintHelperArgs(values)) return values;
  for (const required of local ? ["artifact", "source-sha", "reason"] : REQUIRED) {
    if (!values[required]) throw new Error("required --" + required);
  }
  for (const id of local ? [] : ["run-id", "job-id", "artifact-id"]) {
    if (!/^[1-9][0-9]*$/.test(values[id]))
      throw new Error("--" + id + " must be a positive integer");
  }
  if (!/^[0-9a-f]{40}$/.test(values["source-sha"])) {
    throw new Error("--source-sha must be a full 40-character commit SHA");
  }
  if (!local && Number.isNaN(Date.parse(values["artifact-expires-at"]))) {
    throw new Error("--artifact-expires-at must be an ISO timestamp");
  }
  if (!DARWIN_REASONS.includes(values.reason)) {
    throw new Error("--reason must be one of " + DARWIN_REASONS.join(", "));
  }
  if (Object.hasOwn(values, "stage")) {
    if (!values.stage) throw new Error("--stage needs a directory");
    for (const key of ["contract", "readme"]) {
      if (Object.hasOwn(values, key)) {
        throw new Error("--stage stages the checkout's own files; remove --" + key);
      }
    }
  }
  checkPairedReasonStage(values);
  checkCargoInternalRewireArgs(values);
  checkCargoProducerLockArgs(values);
  checkPreviousContractRefArgs(values, DARWIN_REPINNED_FROM_REASONS);
  return values;
}

function macOsVersion(swVers = "") {
  const field = (name) => new RegExp(name + ":\\s+([^;]+)").exec(swVers)?.[1];
  return field("ProductVersion") + " (" + field("BuildVersion") + ")";
}

export function buildEvidence(args, artifact, local = null) {
  const p = artifact.modes.packed.measurement.provenance;
  const r = artifact.runner;
  return {
    packageVersion: p.packageVersion,
    pnpmLockSha256: p.pnpmLockSha256,
    cargoLockSha256: p.cargoLockSha256,
    dependencyFingerprint: p.dependencyFingerprint,
    fixtureSha256: p.fixtureSha256,
    runnerSha256: p.runnerSha256,
    normalizerSha256: normalizerSha256(),
    sourceSha: args["source-sha"],
    ...(local
      ? { localReceipt: { sha256: local.sha256, receipt: local.receipt } }
      : {
          runId: Number(args["run-id"]),
          jobId: Number(args["job-id"]),
          artifactId: Number(args["artifact-id"]),
          artifactExpiresAt: args["artifact-expires-at"],
        }),
    nativeRunner: {
      platform: "darwin-arm64",
      macOS: macOsVersion(r.sw_vers),
      rustc: r.rustc?.split(";")[0],
      cargo: r.cargo,
      node: r.node,
      zlib: r.zlib,
      pnpm: r.pnpm,
      esbuild: r.esbuild_version,
      hono: r.hono_version,
      zfbBinarySha256: p.zfbBinarySha256,
      esbuildBinarySha256: p.esbuildBinarySha256,
      zfbTarballSha256: p.zfbTarballSha256,
      runtimeTarballSha256: p.runtimeTarballSha256,
      slugifyTarballSha256: artifact.files.slugifyTarballSha256,
    },
    originalCheckerResult: {
      exitCode: Number(artifact.files.checkerExitCode ?? NaN),
      logSha256: artifact.files.checkerLogSha256 ?? null,
    },
  };
}

// A re-pin must describe the checkout it lands in, or the next native check
// on that checkout fails provenance (mirrors the Linux script's checkout check).
// The lockfile comparisons stay byte-exact: an artifact from another checkout
// never becomes acceptable because its dependency graph happens to match.
export function checkoutMismatches(evidence, checkout) {
  const errors = [];
  if (evidence.packageVersion !== checkout.packageVersion) {
    errors.push(
      "artifact measured @takazudo/zfb " +
        evidence.packageVersion +
        ", not the checkout's " +
        checkout.packageVersion,
    );
  }
  for (const [key, file] of [
    ["pnpmLockSha256", "pnpm-lock.yaml"],
    ["cargoLockSha256", "Cargo.lock"],
  ]) {
    if (evidence[key] !== checkout[key]) {
      errors.push("artifact " + file + " hash differs from this checkout's " + file);
    }
  }
  const problem = artifactFingerprintProblem(evidence.dependencyFingerprint, checkout);
  if (problem) errors.push(problem);
  for (const key of ["fixtureSha256", "runnerSha256"]) {
    if (
      checkout[key] !== undefined &&
      !sameJson(sortedEntries(evidence[key]), sortedEntries(checkout[key]))
    ) {
      errors.push("artifact " + key + " differs from this checkout's files");
    }
  }
  return errors;
}

// The checkout's identity, read from `root` with its own loaded helper.
export function readDarwinCheckout(contract, root, helper) {
  return readCheckoutIdentity(contract, root, helper);
}

// What each artifact reason runs on the loaded evidence: `run` returns
// {contract, gzipMoves, previousVersion?}, `readme` renders its README section.
// `checkout` reasons compare the artifact with the checkout and rewrite the
// Darwin test pins; INVENTORY_REASONS regenerate lock-fingerprint-inventory.json
// and fingerprint-policy also rewrites the policy identity line.
const DARWIN_HANDLERS = {
  [METRIC_MIGRATION]: {
    checkout: false,
    run: ({ contract, totals, evidence }) => ({
      contract: migrateDarwinContract(contract, totals, evidence),
      gzipMoves: gzipMovesFromCeilings(contract, totals),
    }),
    readme: (result, { contract }) => darwinReadmeSection(result.contract, contract),
    verb: "migrated",
  },
  ...Object.fromEntries(
    REPIN_REASONS.map((reason) => [
      reason,
      {
        checkout: true,
        run: ({ contract, totals, evidence, checkout }) =>
          repinDarwinContract(contract, totals, evidence, reason, checkout),
        readme: (result) =>
          darwinRepinReadmeSection(
            result.contract,
            result.gzipMoves,
            result.previousVersion,
            reason,
          ),
        verb: "re-pinned",
      },
    ]),
  ),
  [FINGERPRINT_MIGRATION]: {
    checkout: true,
    run: ({ contract, totals, evidence, checkout }) =>
      migrateDarwinFingerprint(contract, totals, evidence, checkout),
    readme: (result) => darwinFingerprintReadmeSection(result.contract, result.gzipMoves),
    verb: "migrated",
  },
  [CARGO_PRODUCER_LOCK_REASON]: {
    checkout: true,
    run: ({ contract, totals, evidence, checkout, args, root }) =>
      transitionDarwinCargoLock(
        contract,
        totals,
        evidence,
        checkout,
        readCargoLockEvidence(args, root),
      ),
    readme: (result) => darwinCargoProducerLockReadmeSection(result.contract, result.gzipMoves),
    verb: "re-pinned",
  },
  [DEPENDENCY_GRAPH_REASON]: {
    checkout: true,
    run: ({ contract, totals, evidence, checkout }) =>
      repinDarwinDependencyGraph(contract, totals, evidence, checkout),
    readme: (result) => darwinDependencyGraphReadmeSection(result.contract, result.gzipMoves),
    verb: "re-pinned",
  },
  [FINGERPRINT_POLICY_REASON]: {
    checkout: true,
    run: ({ contract, totals, evidence, checkout }) =>
      repinDarwinFingerprintPolicy(contract, totals, evidence, checkout),
    readme: (result) => darwinFingerprintPolicyReadmeSection(result.contract, result.gzipMoves),
    verb: "re-pinned",
  },
  [CARGO_INTERNAL_REWIRE_REASON]: {
    checkout: true,
    run: ({ contract, totals, evidence, checkout, args, root }) =>
      transitionDarwinCargoRewire(
        contract,
        totals,
        evidence,
        checkout,
        readCargoRewireEvidence(args, root, DARWIN_PRODUCER_TARGET),
      ),
    readme: (result) => darwinCargoRewireReadmeSection(result.contract, result.gzipMoves),
    verb: "re-pinned",
  },
};

// Actual gzip movement against a schemaVersion-1 contract's actual-gzip ceilings.
function gzipMovesFromCeilings(contract, totals) {
  return MODES.flatMap((mode) =>
    Object.entries(totals[mode])
      .filter(([fixture, row]) => row.gzip !== contract.ceilings[mode][fixture].gzip)
      .map(([fixture, row]) => ({
        mode,
        fixture,
        delta: row.gzip - contract.ceilings[mode][fixture].gzip,
      })),
  );
}

// `root` is the checkout the contract, test pins and README are rewritten in;
// its own lock-fingerprint.mjs computes every fingerprint. Async: the checkout
// helper is imported, and the artifact-less fingerprint-helper reason awaits.
// `stageIo` is openOutputSession's test seam.
export async function main(
  argv = process.argv.slice(2),
  root = ".",
  stageIo = undefined,
  toolRoot = resolve(dirname(fileURLToPath(import.meta.url)), ".."),
) {
  const args = parseArgs(argv);
  root = resolve(args["source-root"] ?? root);
  if (Object.hasOwn(args, "apply")) return applyStage(args.apply, root);
  if (args.reason === FINGERPRINT_HELPER_REASON) {
    return runFingerprintHelperRepin(args["base-ref"], root);
  }
  const local = args["local-receipt"]
    ? readLocalEvidence(
        args["local-receipt"],
        args.artifact,
        args["local-receipt-sha"],
        args["source-sha"],
      )
    : null;
  if (local) {
    verifyLocalCheckout(root, args["source-sha"]);
    verifyLocalCheckout(toolRoot, args["tool-sha"]);
    if (localEvidenceProfile(args.reason).requiresReviewedTransition) {
      if (!args["reviewed-cargo-transition"])
        throw new Error("local evidence requires the exact reviewed security transition");
      args["reviewed-cargo-transition"] = resolve(root, args["reviewed-cargo-transition"]);
    }
    if (args["old-cargo-lock"]) args["old-cargo-lock"] = resolve(root, args["old-cargo-lock"]);
  }
  const handler = DARWIN_HANDLERS[args.reason];
  const session = openOutputSession(root, args.stage, stageIo);
  const contractPath = args.contract === undefined ? CONTRACT : resolve(args.contract);
  const readmePath = args.readme === undefined ? README : resolve(args.readme);
  const contractBytes = readFileSync(resolve(root, contractPath));
  const contract = JSON.parse(contractBytes.toString("utf8"));
  const artifact = loadDarwinArtifact(args.artifact, {
    verifySidecar: SOURCE_BOUND_REASONS.includes(args.reason),
  });
  const problems = validateDarwinEvidence(contract, artifact, args["source-sha"], args.reason);
  if (problems.length > 0) throw new Error("artifact rejected: " + problems.join("; "));
  const totals = measuredTotals({
    workspace: artifact.modes.workspace.recomputed.results,
    packed: artifact.modes.packed.recomputed.results,
  });
  const evidence = buildEvidence(args, artifact, local);
  if (local) evidence.stagingTool = { commit: args["tool-sha"] };
  let checkout = null;
  let helper = null;
  if (handler.checkout) {
    helper = await loadCheckoutHelper(root);
    checkout = readDarwinCheckout(contract, root, helper);
    const mismatches = checkoutMismatches(evidence, checkout);
    if (mismatches.length > 0) throw new Error("re-pin rejected: " + mismatches.join("; "));
  }
  if (DARWIN_REPINNED_FROM_REASONS.includes(args.reason)) {
    evidence.previousContract = previousContractReference(
      root,
      resolve(root, contractPath),
      contractBytes,
      args["previous-contract-ref"],
    );
  }
  const result = handler.run({ contract, totals, evidence, checkout, args, root });
  // Every output is rendered before the first write.
  const readmeInput = existsSync(resolve(root, readmePath))
    ? session.read(readmePath).toString("utf8")
    : "";
  const outputs = [
    { path: contractPath, bytes: JSON.stringify(result.contract, null, 2) + "\n" },
    ...(handler.checkout
      ? [
          {
            path: TEST_FILE,
            bytes: rewriteDarwinTestPins(session.read(TEST_FILE).toString("utf8"), result.contract),
          },
        ]
      : []),
    { path: readmePath, bytes: readmeInput + handler.readme(result, { contract }) },
    ...(INVENTORY_REASONS.includes(args.reason)
      ? [
          {
            path: INVENTORY_FILE,
            bytes: regenerateInventory(
              root,
              result.contract.dependencyFingerprint,
              helper,
              args.reason,
            ),
          },
        ]
      : []),
    ...(args.reason === FINGERPRINT_POLICY_REASON
      ? [
          {
            path: POLICY_DOC,
            bytes: rewritePolicyIdentity(
              session.read(POLICY_DOC).toString("utf8"),
              result.contract.dependencyFingerprint,
            ),
          },
        ]
      : []),
  ];
  const run = {
    reason: args.reason,
    platform: "darwin-arm64",
    sourceSha: args["source-sha"],
    ...(local
      ? {
          stagingTool: { root: resolve(toolRoot), commit: args["tool-sha"] },
          localEvidence: {
            receiptPath: local.receiptPath,
            bundlePath: local.bundlePath,
            sha256: local.sha256,
          },
        }
      : {
          runId: Number(args["run-id"]),
          artifactId: Number(args["artifact-id"]),
        }),
    ...stageRunIdentity(args.reason, contract, result.contract),
  };
  verifyLocalStageEvidence(run, result.contract, root);
  session.commit(outputs, run);
  console.log(
    (args.stage === undefined ? handler.verb : "staged") +
      " Darwin island-size contract to v" +
      result.contract.toolchain.packageVersion +
      " (" +
      args.reason +
      "): " +
      (describeDeltas(result.gzipMoves) || "no actual gzip change") +
      " (actual gzip, evidence only)",
  );
}

const invokedPath = process.argv[1] && resolve(process.argv[1]);
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    await main();
  } catch (error) {
    console.error("rebaseline-island-size-darwin: " + error.message);
    process.exit(1);
  }
}
