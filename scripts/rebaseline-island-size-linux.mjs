#!/usr/bin/env node
// Linux x64 island-size contract re-pin (used by /l-make-release Step 4a and
// by metric migrations, #4131).
//
// The contract measures `gzip-identity-normalized` (research/v3-island-size/
// normalize-identity.mjs): the compiler-owned `zudo_react_build` token is
// replaced with a same-length placeholder before compressing, so a version
// bump or a runtime source edit that only moves the token leaves every raw and
// gzipNormalized total unchanged. This script re-derives the totals from the
// saved artifact bytes (never from measurement.json's claims), refuses any raw
// change, and under `--reason release-bump` / `--reason runtime-identity` /
// `--reason lockfile-provenance` also refuses a gzipNormalized change: a moved
// normalized total is product code moving and needs a reviewed decision, not a
// re-pin. `--reason metric-migration` converts a schemaVersion-1 actual-gzip
// contract to the schemaVersion-2 normalized shape from a replayed artifact.
// `--reason lockfile-provenance` records a consumer-only pnpm-lock.yaml move
// (#4184): the producer version and every other reviewed identity input must be
// unchanged, and only the lock hash may move. `--reason fingerprint-migration`
// (#4187) converts a schemaVersion-2 contract to schemaVersion 3, which pins the
// dependency-graph fingerprint of pnpm-lock.yaml (lock-fingerprint.mjs) instead
// of the whole-file hash: the producer version, normalizer, fixtures, Cargo.lock
// and toolchain must be unchanged, and only the runner digests (which gained the
// fingerprint) and the recorded lock hash may follow the measured artifact.

import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { sameHashMaps } from "../research/v3-island-size/check-budget.mjs";
import { computeLockFingerprint } from "../research/v3-island-size/lock-fingerprint.mjs";
import {
  IDENTITY_NORMALIZATION_VERSION,
  IDENTITY_NORMALIZED_METRIC,
  normalizeMeasurementDirectory,
  normalizerSha256,
} from "../research/v3-island-size/normalize-identity.mjs";

const MODES = ["workspace", "packed"];
const SIZE_DIR = "research/v3-island-size";
const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
const README = "research/v3-island-size/README.md";
const TEST_FILE = "scripts/__tests__/island-size-budget.test.mjs";
const TEST_ANCHOR = 'it("records the reviewed Linux x64 ceilings with zero allowance"';

export const REASONS = Object.freeze({
  "release-bump": {
    outcome: "release-version-stamp",
    heading: "release baseline",
    requires: 2,
    accepts: [2, 3],
  },
  "metric-migration": {
    outcome: "metric-migration",
    heading: "metric migration",
    requires: 1,
  },
  "runtime-identity": {
    outcome: "runtime-identity-repin",
    heading: "runtime-identity re-pin",
    requires: 2,
    accepts: [2, 3],
  },
  "lockfile-provenance": {
    outcome: "lockfile-provenance-repin",
    heading: "lockfile-provenance re-pin",
    requires: 2,
  },
  "fingerprint-migration": {
    outcome: "fingerprint-migration",
    heading: "dependency-fingerprint migration",
    requires: 2,
  },
});

// Contract schema versions a reason may rewrite; `requires` is the oldest.
export function acceptedSchemas(reason) {
  const policy = REASONS[reason];
  return policy.accepts ?? [policy.requires];
}

// Both identity-normalized schemas carry {raw, gzipNormalized} ceilings.
function normalizedSchema(contract) {
  return contract.schemaVersion === 2 || contract.schemaVersion === 3;
}

const CONTRACT_FINGERPRINT_KEYS = ["policyVersion", "parser", "helperSha256", "sha256"];
const PROVENANCE_FINGERPRINT_KEYS = ["policyVersion", "sha256"];

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

// The schemaVersion-3 `dependencyFingerprint` block for a checkout, in contract
// key order. Throws (fail-closed) when lock-fingerprint.mjs rejects the lock.
export function checkoutFingerprint(lockText, currentVersion, repoRoot) {
  const fingerprint = computeLockFingerprint({
    lockText,
    currentVersion,
    ...(repoRoot ? { repoRoot } : {}),
  });
  return contractFingerprint(fingerprint);
}

// The four contract keys of a fingerprint, in contract key order.
export function contractFingerprint(fingerprint) {
  return Object.fromEntries(CONTRACT_FINGERPRINT_KEYS.map((key) => [key, fingerprint[key]]));
}

// The checkout's own fixture and runner digests, keyed like the contract maps,
// so an artifact is compared with what this tree would measure.
export function checkoutIdentityHashes(contract, root = ".") {
  const hashMap = (keys, path) =>
    Object.fromEntries(keys.map((key) => [key, sha256(readFileSync(join(root, path(key))))]));
  return {
    fixtureSha256: hashMap(Object.keys(contract.fixtureSha256), (name) =>
      join(SIZE_DIR, "fixtures", name + ".tsx"),
    ),
    runnerSha256: hashMap(Object.keys(contract.runnerSha256), (name) => join(SIZE_DIR, name)),
  };
}

// The checkout's identity, read from `root`. A fingerprint failure is kept as
// dependencyFingerprintError and reported by artifactFingerprintProblem.
export function readCheckoutIdentity(contract, root = ".") {
  const at = (path) => join(root, path);
  const packageVersion = JSON.parse(readFileSync(at("packages/zfb/package.json"), "utf8")).version;
  const lockBytes = readFileSync(at("pnpm-lock.yaml"));
  const checkout = {
    packageVersion,
    pnpmLockSha256: sha256(lockBytes),
    cargoLockSha256: sha256(readFileSync(at("Cargo.lock"))),
    ...checkoutIdentityHashes(contract, root),
    dependencyFingerprint: null,
    dependencyFingerprintError: null,
  };
  try {
    checkout.dependencyFingerprint = checkoutFingerprint(
      lockBytes.toString("utf8"),
      packageVersion,
      resolve(root),
    );
  } catch (error) {
    checkout.dependencyFingerprintError = error?.message ?? String(error);
  }
  return checkout;
}

// Every reason the artifact's run provenance does not describe this checkout
// byte-for-byte, or []. A migration relaxes runner equality against the old
// contract only, never against the measured checkout.
export function artifactCheckoutMismatches(provenance, checkout, label) {
  const errors = [];
  if (provenance.pnpmLockSha256 !== checkout.pnpmLockSha256) {
    errors.push(label + " artifact lockfile hash differs from this checkout's pnpm-lock.yaml");
  }
  if (provenance.packageVersion !== checkout.packageVersion) {
    errors.push(
      label +
        " artifact measured @takazudo/zfb " +
        provenance.packageVersion +
        ", not " +
        checkout.packageVersion,
    );
  }
  if (provenance.cargoLockSha256 !== checkout.cargoLockSha256) {
    errors.push(label + " artifact Cargo.lock hash differs from this checkout's Cargo.lock");
  }
  for (const key of ["fixtureSha256", "runnerSha256"]) {
    if (!sameHashMaps(checkout[key], provenance[key])) {
      errors.push(label + " artifact " + key + " differs from this checkout's files");
    }
  }
  const problem = artifactFingerprintProblem(provenance.dependencyFingerprint, checkout);
  if (problem) errors.push(label + " " + problem);
  return errors;
}

// Why an artifact's {policyVersion, sha256} cannot stand for the checkout's
// fingerprint, or null. An artifact from a different checkout never becomes
// acceptable merely because its whole-file lock hash is ignored.
export function artifactFingerprintProblem(artifactFingerprint, checkout) {
  if (checkout.dependencyFingerprintError) {
    return "checkout dependency fingerprint failed: " + checkout.dependencyFingerprintError;
  }
  const local = checkout.dependencyFingerprint;
  if (!local) return "checkout dependency fingerprint is unavailable";
  if (
    artifactFingerprint === null ||
    typeof artifactFingerprint !== "object" ||
    JSON.stringify(Object.keys(artifactFingerprint).sort()) !==
      JSON.stringify([...PROVENANCE_FINGERPRINT_KEYS].sort())
  ) {
    return (
      "artifact provenance must carry dependencyFingerprint with exactly " +
      PROVENANCE_FINGERPRINT_KEYS.join(" and ")
    );
  }
  const moved = PROVENANCE_FINGERPRINT_KEYS.filter(
    (key) => artifactFingerprint[key] !== local[key],
  );
  return moved.length === 0
    ? null
    : "artifact dependency fingerprint " + moved.join(", ") + " differs from this checkout's";
}

function contractFingerprintMoves(contract, local) {
  return CONTRACT_FINGERPRINT_KEYS.filter(
    (key) => contract.dependencyFingerprint?.[key] !== local?.[key],
  );
}

// fingerprint-migration may move only the runner digests and the recorded lock
// hash; every other reviewed identity input must equal the contract.
export function assertFingerprintMigrationMovement(contract, evidence) {
  const identity = evidence.identity;
  if (!identity) {
    throw new Error("--reason fingerprint-migration needs the artifact's measured identity inputs");
  }
  const problem = artifactFingerprintProblem(evidence.artifactFingerprint, evidence);
  if (problem) throw new Error("--reason fingerprint-migration: " + problem);
  const moved = [];
  if (evidence.version !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (evidence.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  if (!sameHashMaps(identity.fixtureSha256, contract.fixtureSha256)) moved.push("fixtureSha256");
  if (identity.cargoLockSha256 !== contract.toolchain.cargoLockSha256) {
    moved.push("cargoLockSha256");
  }
  for (const key of IDENTITY_TOOLCHAIN_KEYS) {
    if (identity.toolchain?.[key] !== contract.toolchain[key]) moved.push("toolchain." + key);
  }
  if (moved.length > 0) {
    throw new Error(
      "--reason fingerprint-migration changes only the lock pin, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change first",
    );
  }
}

// release-bump / runtime-identity on a schemaVersion-3 contract: the measured
// artifact must carry this checkout's fingerprint and the contract's must not move.
export function assertFingerprintUnchanged(contract, evidence, reason) {
  const problem = artifactFingerprintProblem(evidence.artifactFingerprint, evidence);
  if (problem) throw new Error("--reason " + reason + ": " + problem);
  const moved = contractFingerprintMoves(contract, evidence.dependencyFingerprint);
  if (moved.length > 0) {
    throw new Error(
      "--reason " +
        reason +
        " cannot move the dependency fingerprint (" +
        moved.join(", ") +
        " differ from the contract); a dependency-graph change needs a reviewed decision, not a " +
        reason +
        " re-pin",
    );
  }
}

const IDENTITY_TOOLCHAIN_KEYS = ["honoVersion", "esbuildVersion", "nodeVersion", "zlibVersion"];

// The reviewed identity a measurement artifact carries, in contract shape.
export function measuredIdentity(provenance) {
  return {
    fixtureSha256: provenance.fixtureSha256,
    runnerSha256: provenance.runnerSha256,
    cargoLockSha256: provenance.cargoLockSha256,
    toolchain: {
      honoVersion: provenance.honoPackageVersion,
      esbuildVersion: provenance.esbuildVersion,
      nodeVersion: provenance.nodeVersion,
      zlibVersion: provenance.zlibVersion,
    },
  };
}

// lockfile-provenance may move only pnpmLockSha256: anything else that the
// contract pins must equal the measured artifact and this checkout.
export function assertLockfileOnlyMovement(contract, evidence) {
  const moved = [];
  if (evidence.version !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (evidence.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  const identity = evidence.identity;
  if (!identity) {
    throw new Error("--reason lockfile-provenance needs the artifact's measured identity inputs");
  }
  if (!sameHashMaps(identity.fixtureSha256, contract.fixtureSha256)) moved.push("fixtureSha256");
  if (!sameHashMaps(identity.runnerSha256, contract.runnerSha256)) moved.push("runnerSha256");
  if (identity.cargoLockSha256 !== contract.toolchain.cargoLockSha256) {
    moved.push("cargoLockSha256");
  }
  for (const key of IDENTITY_TOOLCHAIN_KEYS) {
    if (identity.toolchain?.[key] !== contract.toolchain[key]) moved.push("toolchain." + key);
  }
  if (moved.length > 0) {
    throw new Error(
      "--reason lockfile-provenance allows only pnpm-lock.yaml to move, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change",
    );
  }
  if (evidence.pnpmLockSha256 === contract.toolchain.pnpmLockSha256) {
    throw new Error(
      "--reason lockfile-provenance found the contract's pnpm-lock.yaml hash unchanged; nothing to re-pin",
    );
  }
}

// `normalizedByMode` holds normalize-identity.mjs results recomputed from
// the saved bytes, keyed by mode. Actual gzip rides along as evidence only.
export function measuredTotals(normalizedByMode) {
  const totals = {};
  for (const mode of MODES) {
    totals[mode] = {};
    for (const [fixture, result] of Object.entries(normalizedByMode[mode])) {
      totals[mode][fixture] = {
        raw: result.shipped.raw,
        gzipNormalized: result.shipped.gzipNormalized,
        gzip: result.shipped.gzip,
      };
    }
  }
  return totals;
}

export function assertLinuxX64Provenance(provenance) {
  if (provenance?.platform?.os !== "linux" || provenance?.platform?.arch !== "x64") {
    throw new Error(
      "artifact was measured on " +
        JSON.stringify(provenance?.platform) +
        ", not linux-x64; Darwin arm64 uses rebaseline-island-size-darwin.mjs",
    );
  }
}

export function diffCeilings(contract, totals) {
  const missingFixtures = [];
  const rawChanges = [];
  const normalizedChanges = [];
  const gzipChanges = [];
  const normalizedContract = normalizedSchema(contract);
  for (const mode of MODES) {
    const ceilings = contract.ceilings[mode];
    const fixtures = new Set([...Object.keys(ceilings), ...Object.keys(totals[mode])]);
    for (const fixture of fixtures) {
      const before = ceilings[fixture];
      const after = totals[mode][fixture];
      if (!before || !after) {
        missingFixtures.push(mode + "/" + fixture);
      } else if (before.raw !== after.raw) {
        rawChanges.push({ mode, fixture, before, after });
      } else if (normalizedContract && before.gzipNormalized !== after.gzipNormalized) {
        normalizedChanges.push({
          mode,
          fixture,
          delta: after.gzipNormalized - before.gzipNormalized,
        });
      } else if (!normalizedContract && before.gzip !== after.gzip) {
        gzipChanges.push({ mode, fixture, delta: after.gzip - before.gzip });
      }
    }
  }
  return { missingFixtures, rawChanges, normalizedChanges, gzipChanges };
}

function signed(n) {
  return n > 0 ? "+" + n : String(n);
}

function joinNames(names) {
  return names.length < 2 ? names.join("") : names.slice(0, -1).join(", ") + " and " + names.at(-1);
}

export function describeDeltas(changes) {
  const parts = [];
  for (const mode of MODES) {
    const byDelta = new Map();
    for (const change of changes.filter((c) => c.mode === mode)) {
      byDelta.set(change.delta, [...(byDelta.get(change.delta) ?? []), change.fixture]);
    }
    const groups = [...byDelta.entries()]
      .sort(([a], [b]) => b - a)
      .map(([delta, fixtures]) => joinNames(fixtures) + " " + signed(delta));
    if (groups.length > 0) parts.push(mode + " " + groups.join(", "));
  }
  return parts.join("; ");
}

// health.yml measures a PR's synthetic merge commit, which no branch keeps; the
// probe head (still reachable as refs/pull/N/head) has the identical tree.
function sourceText(measuredSha, headSha) {
  return measuredSha === headSha
    ? "source " + headSha
    : "PR merge source " + measuredSha + " (probe head " + headSha + ")";
}

function deltaRange(changes) {
  const deltas = changes.map((c) => c.delta);
  return signed(Math.min(...deltas)) + " to " + signed(Math.max(...deltas));
}

function metricText() {
  return IDENTITY_NORMALIZED_METRIC + " v" + IDENTITY_NORMALIZATION_VERSION;
}

// Actual gzip movement between the previous contract and the new measurement,
// reported as transfer-size evidence; never a ceiling under schemaVersion 2/3.
// schemaVersion 1 ceilings are actual gzip; schemaVersion 2/3 ceilings carry only
// gzipNormalized, so the previous actual gzip is read from the evidence a
// re-pin records in baselineEvidence.actualGzip. Null when none was recorded.
export function actualGzipEvidence(contract, totals) {
  const changes = [];
  for (const mode of MODES) {
    for (const [fixture, after] of Object.entries(totals[mode])) {
      const before = normalizedSchema(contract)
        ? contract.baselineEvidence?.actualGzip?.[mode]?.[fixture]
        : contract.ceilings[mode][fixture]?.gzip;
      if (typeof before !== "number") return null;
      if (before !== after.gzip) changes.push({ mode, fixture, delta: after.gzip - before });
    }
  }
  return changes;
}

function actualGzipRows(totals) {
  return Object.fromEntries(
    MODES.map((mode) => [
      mode,
      Object.fromEntries(Object.entries(totals[mode]).map(([name, row]) => [name, row.gzip])),
    ]),
  );
}

export function rebaselineContract(contract, totals, evidence, reason) {
  const policy = REASONS[reason];
  if (!policy) throw new Error("unknown --reason " + String(reason));
  const schemas = acceptedSchemas(reason);
  if (!schemas.includes(contract.schemaVersion)) {
    throw new Error(
      "--reason " +
        reason +
        " needs a schemaVersion " +
        schemas.join(" or ") +
        " contract, found schemaVersion " +
        String(contract.schemaVersion),
    );
  }
  const { missingFixtures, rawChanges, normalizedChanges } = diffCeilings(contract, totals);
  if (missingFixtures.length > 0) {
    throw new Error(
      "fixture set differs from the contract (" +
        missingFixtures.join(", ") +
        "); refusing to rebaseline",
    );
  }
  if (rawChanges.length > 0) {
    const rows = rawChanges.map((c) => c.mode + "/" + c.fixture).join(", ");
    throw new Error(
      "raw totals changed (" +
        rows +
        "); this is not a " +
        reason +
        " re-pin — record a reviewed decision instead",
    );
  }
  if (normalizedChanges.length > 0) {
    const rows = normalizedChanges.map((c) => c.mode + "/" + c.fixture + " " + signed(c.delta));
    throw new Error(
      "gzipNormalized totals changed (" +
        rows.join(", ") +
        "); the identity token is normalized away, so this is product code moving, not a " +
        reason +
        " — record a reviewed decision instead",
    );
  }
  if (reason === "lockfile-provenance") assertLockfileOnlyMovement(contract, evidence);
  if (reason === "fingerprint-migration") assertFingerprintMigrationMovement(contract, evidence);
  if (contract.schemaVersion === 3) assertFingerprintUnchanged(contract, evidence, reason);
  const previousVersion = contract.toolchain.packageVersion;
  const gzipEvidence = actualGzipEvidence(contract, totals);
  const next = structuredClone(contract);
  if (reason === "metric-migration") {
    const migrated = {};
    for (const [key, value] of Object.entries(next)) {
      migrated[key] = value;
      if (key === "schemaVersion") {
        migrated.schemaVersion = 2;
        migrated.metric = IDENTITY_NORMALIZED_METRIC;
        migrated.normalizationVersion = IDENTITY_NORMALIZATION_VERSION;
        migrated.normalizerSha256 = evidence.normalizerSha256;
      }
    }
    Object.keys(next).forEach((key) => delete next[key]);
    Object.assign(next, migrated);
    next.allowance = { raw: 0, gzipNormalized: 0 };
  }
  if (reason === "fingerprint-migration") {
    const migrated = {};
    for (const [key, value] of Object.entries(next)) {
      migrated[key] = value;
      if (key === "schemaVersion") migrated.schemaVersion = 3;
      if (key === "normalizerSha256") {
        migrated.dependencyFingerprint = contractFingerprint(evidence.dependencyFingerprint);
      }
    }
    Object.keys(next).forEach((key) => delete next[key]);
    Object.assign(next, migrated);
    next.runnerSha256 = { ...evidence.identity.runnerSha256 };
  }
  next.outcome = policy.outcome;
  next.baselineSourceSha = evidence.headSha;
  next.toolchain.packageVersion = evidence.version;
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  next.normalizerSha256 = evidence.normalizerSha256;
  const gzipSummary =
    gzipEvidence === null
      ? "no previous actual gzip evidence to compare"
      : gzipEvidence.length === 0
        ? "no actual gzip total moved"
        : gzipEvidence.length +
          " actual gzip totals moved by " +
          deltaRange(gzipEvidence) +
          " bytes (evidence only, not a ceiling)";
  const matrices =
    " Workspace and packed eight-fixture matrices each have two byte-identical full-dist " +
    "inventories. Other platforms and toolchains are not covered.";
  if (reason === "metric-migration") {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", replayed by normalize-identity.mjs over the retained artifact of health run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      ". All 16 raw totals are unchanged from the v" +
      previousVersion +
      " actual-gzip contract; the gzipNormalized ceilings are new values computed from the " +
      "saved bytes and are not comparable with any earlier gzip ceiling. Integrated-tree CI " +
      "re-verification by the manager is pending." +
      matrices;
  } else if (reason === "release-bump") {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from health run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      ". Every raw and gzipNormalized total is unchanged from the v" +
      previousVersion +
      " contract. This records a release-bump re-pin; equal totals alone do not prove byte-identical product code. " +
      gzipSummary +
      "." +
      matrices;
  } else if (reason === "fingerprint-migration") {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", migrated to schemaVersion 3 from workflow run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      ". The contract now pins the dependency-graph fingerprint of pnpm-lock.yaml (policy v" +
      evidence.dependencyFingerprint.policyVersion +
      ", " +
      evidence.dependencyFingerprint.parser +
      ") instead of the whole-file hash, which stays recorded as baseline provenance only. The producer version, fixtures, Cargo.lock, toolchain and normalizer are unchanged; the runner digests follow the measured artifact because the runners now record the fingerprint. Every raw and gzipNormalized total is unchanged. " +
      gzipSummary +
      "." +
      matrices;
  } else if (reason === "lockfile-provenance") {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from health run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      " after a consumer-only pnpm-lock.yaml change. The producer version, fixtures, runners, Cargo.lock, toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; only the lock hash moved. This is not a release or runtime change. " +
      gzipSummary +
      "." +
      matrices;
  } else {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from health run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      ". Every raw and gzipNormalized total is unchanged. This records a runtime-identity re-pin; equal totals alone do not prove byte-identical product code. " +
      gzipSummary +
      "." +
      matrices;
  }
  next.baselineEvidence = {
    event: reason === "metric-migration" ? "ci-artifact-replay" : "ci",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    ...(reason === "fingerprint-migration"
      ? {
          migratedFrom: {
            schemaVersion: contract.schemaVersion,
            outcome: contract.outcome,
            baselineSourceSha: contract.baselineSourceSha,
            baselineRunId: contract.baselineEvidence?.runId,
          },
        }
      : {}),
    actualGzip: actualGzipRows(totals),
    note:
      evidence.runLabel +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      ". Complete Linux x64 workspace and packed eight-fixture matrices, each " +
      "with two identical full-dist inventories. " +
      (reason === "metric-migration"
        ? "Metric migration: the gzipNormalized ceilings were computed by normalize-identity.mjs v" +
          IDENTITY_NORMALIZATION_VERSION +
          " over the saved artifact bytes, not converted arithmetically from the v" +
          previousVersion +
          " actual-gzip ceilings. Raw totals did not change. Actual gzip stays in the sidecar " +
          "as transfer-size evidence. The manager re-verifies these values on the integrated " +
          "tree in CI."
        : reason === "fingerprint-migration"
          ? "Dependency-fingerprint migration (schemaVersion 2 to 3) at unchanged v" +
            previousVersion +
            ": the fingerprint " +
            evidence.dependencyFingerprint.sha256 +
            " was computed by lock-fingerprint.mjs on the checkout and equals the artifact's; the pnpm-lock.yaml hash " +
            evidence.pnpmLockSha256 +
            " is the artifact's own lock, recorded as provenance only. Raw and gzipNormalized totals did not change; " +
            gzipSummary +
            "."
          : reason === "lockfile-provenance"
            ? "Consumer-only pnpm-lock.yaml move at unchanged v" +
              previousVersion +
              " with every other reviewed identity input unchanged; raw and gzipNormalized totals did not change; " +
              gzipSummary +
              "."
            : "Raw and gzipNormalized totals did not change from the v" +
              previousVersion +
              " contract; " +
              gzipSummary +
              ".") +
      " Ceilings equal the artifact measurements with zero allowance; earlier ceilings remain in git history.",
  };
  for (const mode of MODES) {
    for (const fixture of Object.keys(next.ceilings[mode])) {
      next.ceilings[mode][fixture] = {
        raw: totals[mode][fixture].raw,
        gzipNormalized: totals[mode][fixture].gzipNormalized,
      };
    }
  }
  return { contract: next, gzipEvidence, previousVersion };
}

function fixtureKey(fixture) {
  return /^[a-z]+$/.test(fixture) ? fixture : JSON.stringify(fixture);
}

// Rewrites one `it(...)` block of the budget test: `edit` maps the block text
// to its replacement and everything outside the block is returned untouched.
export function rewriteTestBlock(source, anchor, label, edit) {
  const start = source.indexOf(anchor);
  if (start < 0) throw new Error(label + " test not found in " + TEST_FILE);
  const end = source.indexOf("\n  });", start);
  return source.slice(0, start) + edit(source.slice(start, end)) + source.slice(end);
}

export function rewriteTestPins(source, contract) {
  return rewriteTestBlock(source, TEST_ANCHOR, "Linux ceiling", (block) =>
    rewriteLinuxBlock(block, contract),
  );
}

function rewriteLinuxBlock(block, contract) {
  block = block.replace(
    /(linuxContract\.toolchain\.packageVersion\)\.toBe\()"[^"]+"/,
    '$1"' + contract.toolchain.packageVersion + '"',
  );
  block = block.replace(
    /(linuxContract\.schemaVersion\)\.toBe\()\d+/,
    "$1" + contract.schemaVersion,
  );
  for (const mode of MODES) {
    const marker = "linuxContract.ceilings." + mode + ").toEqual({";
    const blockStart = block.indexOf(marker);
    const blockEnd = block.indexOf("});", blockStart);
    if (blockStart < 0 || blockEnd < 0)
      throw new Error("missing " + mode + " pins in " + TEST_FILE);
    const lines = Object.entries(contract.ceilings[mode]).map(
      ([fixture, c]) =>
        "      " +
        fixtureKey(fixture) +
        ": { raw: " +
        c.raw +
        ", gzipNormalized: " +
        c.gzipNormalized +
        " },",
    );
    block =
      block.slice(0, blockStart + marker.length) +
      "\n" +
      lines.join("\n") +
      "\n    " +
      block.slice(blockEnd);
  }
  return block;
}

export function readmeSection(contract, gzipEvidence, previousVersion, reason) {
  const { runId, jobId, sourceSha } = contract.baselineEvidence;
  const policy = REASONS[reason];
  const version = contract.toolchain.packageVersion;
  const runKind = reason === "fingerprint-migration" ? "workflow run " : "health run ";
  const run =
    "[" +
    runKind +
    runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    runId +
    ") (job " +
    jobId +
    ") at " +
    sourceText(sourceSha, contract.baselineSourceSha).replace(/([0-9a-f]{40})/g, "`$1`");
  const evidence =
    gzipEvidence === null
      ? "No previous actual gzip evidence was recorded to compare."
      : gzipEvidence.length === 0
        ? "No actual gzip total moved."
        : "Actual gzip moved on " +
          gzipEvidence.length +
          " rows by " +
          deltaRange(gzipEvidence) +
          " bytes (" +
          describeDeltas(gzipEvidence) +
          "); that is transfer-size evidence in the sidecar, not a ceiling.";
  const closing =
    " The retained `island-size-linux-x64` artifact contains all eight fixtures in both modes with two " +
    "identical full-dist inventories per case. `decision-linux-x64.json` records the `raw` and " +
    "`gzipNormalized` totals as exact ceilings with zero allowance; earlier ceilings remain in git " +
    "history. The Darwin arm64 contract is unchanged; it is re-pinned separately from a native " +
    "`island-size-darwin.yml` artifact by `rebaseline-island-size-darwin.mjs`.\n";
  let body;
  if (reason === "metric-migration") {
    body =
      "The Linux contract moved from actual gzip (schemaVersion 1) to `" +
      metricText() +
      "` (schemaVersion 2) by replaying `normalize-identity.mjs` over the retained artifact of " +
      run +
      ". Every raw total is unchanged from the v" +
      previousVersion +
      " contract; the `gzipNormalized` ceilings are computed from the saved bytes and are not " +
      "comparable with any earlier `gzip` ceiling. " +
      evidence +
      " The manager re-verifies these ceilings on the integrated tree in CI." +
      closing;
  } else if (reason === "release-bump") {
    body =
      "The Linux contract was re-pinned for the v" +
      version +
      " release from " +
      run +
      ". Every raw and `gzipNormalized` total is unchanged from the v" +
      previousVersion +
      " contract. These totals alone do not prove byte-identical product code. " +
      evidence +
      closing;
  } else if (reason === "fingerprint-migration") {
    const fingerprint = contract.dependencyFingerprint;
    body =
      "The Linux contract moved from schemaVersion 2 to schemaVersion 3 at v" +
      version +
      " from " +
      run +
      ". It now pins the dependency-graph fingerprint `" +
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
      "` stays recorded in `toolchain.pnpmLockSha256` as baseline provenance only. The producer " +
      "version, fixtures, `Cargo.lock`, toolchain and normalizer are unchanged, the runner digests " +
      "follow the measured artifact because the runners now record the fingerprint, and every raw " +
      "and `gzipNormalized` total is unchanged. " +
      evidence +
      closing;
  } else if (reason === "lockfile-provenance") {
    body =
      "The Linux contract was re-pinned at v" +
      version +
      " from " +
      run +
      " for a consumer-only `pnpm-lock.yaml` change: the producer version, fixtures, runners, " +
      "`Cargo.lock`, toolchain and normalizer are unchanged, and every raw and `gzipNormalized` " +
      "total is unchanged. Only the reviewed lock hash moved; this is not a release or runtime " +
      "change. " +
      evidence +
      closing;
  } else {
    body =
      "The Linux contract was re-pinned at v" +
      version +
      " from " +
      run +
      " for a runtime-identity change: every raw and `gzipNormalized` total is unchanged. " +
      "These totals alone do not prove byte-identical product code. " +
      evidence +
      closing;
  }
  return "\n## v" + version + " " + policy.heading + "\n\n" + body;
}

export function parseArgs(argv) {
  const values = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined)
      throw new Error("bad argument: " + argv[i]);
    values[argv[i].slice(2)] = argv[i + 1];
  }
  for (const required of ["artifact", "run-id", "job-id", "head-sha", "reason"]) {
    if (!values[required]) throw new Error("required --" + required);
  }
  for (const id of ["run-id", "job-id"]) {
    if (!/^[1-9][0-9]*$/.test(values[id]))
      throw new Error("--" + id + " must be a positive integer");
  }
  if (!/^[0-9a-f]{40}$/.test(values["head-sha"])) {
    throw new Error("--head-sha must be a full 40-character commit SHA");
  }
  if (!Object.hasOwn(REASONS, values.reason)) {
    throw new Error("--reason must be one of " + Object.keys(REASONS).join(", "));
  }
  return values;
}

// `root` is the checkout the contract, test pins and README are rewritten in.
export function main(argv = process.argv.slice(2), root = ".") {
  const args = parseArgs(argv);
  const at = (path) => join(root, path);
  const measurements = {};
  const normalized = {};
  for (const mode of MODES) {
    const { sidecar } = normalizeMeasurementDirectory(join(args.artifact, "real-" + mode));
    normalized[mode] = sidecar.results;
    measurements[mode] = JSON.parse(
      readFileSync(join(args.artifact, "real-" + mode, "measurement.json"), "utf8"),
    );
  }
  const provenance = measurements.workspace.provenance;
  for (const mode of MODES) assertLinuxX64Provenance(measurements[mode].provenance);
  const contract = JSON.parse(readFileSync(at(CONTRACT), "utf8"));
  const checkout = readCheckoutIdentity(contract, root);
  const { packageVersion: version, pnpmLockSha256 } = checkout;
  for (const mode of MODES) {
    const p = measurements[mode].provenance;
    if (p.sourceSha !== provenance.sourceSha)
      throw new Error("workspace and packed source SHAs differ");
    if (JSON.stringify(measuredIdentity(p)) !== JSON.stringify(measuredIdentity(provenance))) {
      throw new Error("workspace and packed identity inputs differ");
    }
    const mismatches = artifactCheckoutMismatches(p, checkout, mode);
    if (mismatches.length > 0) throw new Error(mismatches.join("; "));
  }
  const defaultLabel = {
    "release-bump": "Release probe run for v" + version,
    "metric-migration":
      "Metric migration replayed from the retained artifact of health run " + args["run-id"],
    "runtime-identity": "Runtime-identity re-pin run at v" + version,
    "lockfile-provenance": "Consumer lockfile re-pin run at v" + version,
    "fingerprint-migration": "Dependency-fingerprint migration run at v" + version,
  }[args.reason];
  const result = rebaselineContract(
    contract,
    measuredTotals(normalized),
    {
      version,
      pnpmLockSha256,
      normalizerSha256: normalizerSha256(),
      sourceSha: provenance.sourceSha,
      headSha: args["head-sha"],
      runId: Number(args["run-id"]),
      jobId: Number(args["job-id"]),
      runLabel: args.label ?? defaultLabel,
      identity: measuredIdentity(provenance),
      artifactFingerprint: provenance.dependencyFingerprint,
      dependencyFingerprint: checkout.dependencyFingerprint,
      dependencyFingerprintError: checkout.dependencyFingerprintError,
    },
    args.reason,
  );
  writeFileSync(at(CONTRACT), JSON.stringify(result.contract, null, 2) + "\n");
  writeFileSync(
    at(TEST_FILE),
    rewriteTestPins(readFileSync(at(TEST_FILE), "utf8"), result.contract),
  );
  appendFileSync(
    at(README),
    readmeSection(result.contract, result.gzipEvidence, result.previousVersion, args.reason),
  );
  console.log(
    "re-pinned Linux island-size contract to v" +
      version +
      " (" +
      args.reason +
      "): " +
      (describeDeltas(result.gzipEvidence ?? []) || "no actual gzip change") +
      " (actual gzip, evidence only)",
  );
}

const invokedPath = process.argv[1] && resolve(process.argv[1]);
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error("rebaseline-island-size-linux: " + error.message);
    process.exit(1);
  }
}
