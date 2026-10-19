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
// `--reason fingerprint-helper --base-ref <ref>` (#4198) takes no artifact and
// runs nothing natively: it re-pins only `dependencyFingerprint.helperSha256` of
// BOTH contracts after a comment/formatting edit to lock-fingerprint.mjs, once it
// has proven that the pinned helper at <ref> and the new helper compute the same
// fingerprint and inventory on the real lock (repinFingerprintHelper below; the
// Darwin script reuses it). A helper edit that moves the fingerprint is refused.
// `--reason cargo-producer-lock` (#4214, schemaVersion 3) records one reviewed
// Cargo.lock move from a fresh artifact, additive by default: it binds the old and new lock
// hashes and an explicit `name@version` allowlist, proves every old
// `[[package]]` record survives byte-identical and that the additions are
// exactly the allowlist (proveCargoProducerLock below; the Darwin script reuses
// it), and moves only the Cargo hash and its evidence. `release-bump` and
// `runtime-identity` refuse any Cargo.lock movement and name this reason.
// The old lock is read from git, never from a filesystem path (#4219):
// `--old-cargo-ref <ref>[:<path>]` (path defaults to Cargo.lock) is resolved
// once to a full commit in the supplied checkout root, the blob is read with
// `git show <commit>:<path>`, its raw bytes must hash to
// `--expected-old-cargo-sha`, and the transition records
// `oldLockSource: {commit, path, sha256}` (readGitBlob below). An optional
// `--old-cargo-lock <file>` only corroborates: its bytes must equal the blob.
//
// Every artifact reason except `metric-migration` and `fingerprint-migration`
// (the latter keeps `migratedFrom`) records the immediate predecessor as a flat
// `baselineEvidence.repinnedFrom` (predecessorRecord below, shared with the
// Darwin script), never nesting the previous evidence. Its `contract` member
// cites the previous contract bytes: always their SHA-256, plus `commit` and
// `path` when `git show HEAD:<path>` (or an explicit `--previous-contract-ref`,
// which must match and never falls back to HEAD) is byte-identical to them.
// `baselineSourceSha` names the probe head (`--head-sha`) and
// `baselineEvidence.sourceSha` the measured source, which differ when
// health.yml measured a PR merge commit (sourceText); `contract.commit` names
// the commit that stores the previous contract file. They are different commits.
//
// `--reason cargo-internal-rewire` (#4258, schemaVersion 3) records one reviewed
// Cargo.lock move that `cargo-producer-lock` cannot: an unpublished workspace
// crate is added and/or an existing unpublished workspace record's dependency
// edges are rewired, while every other record stays byte-identical. The
// transition is declared up front in a reviewed JSON file
// (`--rewire-declaration`, REWIRE_DECLARATION_SCHEMA below) that binds the old
// and new lock SHA-256, every added record's exact bytes and every changed
// record's exact before/after bytes and dependency-edge delta; the declaration
// is proposed evidence that proveCargoInternalRewire checks against the actual
// blobs, never policy approval in itself. Internal status is proven from the
// referenced workspace manifests in the checkout (`publish = false`, matching
// name/version, a workspace member path) and from the record itself (no
// `source`, no `checksum`), never from a name prefix. Two producer dependency
// closures (`--old-closure`, `--new-closure`, written by
// scripts/record-cargo-producer-closure.mjs at each lock for this platform's
// target) must differ only in the declared internal rows: any external
// package, version, source or feature drift is refused. The old lock comes
// from git (`--old-cargo-ref`), the new lock is the checkout's, optionally
// cited at `--new-cargo-ref`. Only CARGO_INTERNAL_REWIRE_MOVEMENT is written;
// totals, ceilings, allowance, metric, normalizer, fixtures, runners,
// toolchain, pnpm fingerprint and package version must be unchanged. It is a
// paired, source-bound reason: `--source-sha` and `--stage` are required, and
// `--apply` lands both platforms together once their transitions and
// declaration digests agree (pairedRunsProblem).
// Declaration schema 2 is a separate, narrower shape: existing unpublished
// internal records only gain explicitly declared edges to already-present,
// byte-identical external records. Both endpoints must already belong to the
// default-producer closure; no record/closure additions or edge removals are
// allowed. External inventory and features remain unchanged. See
// research/v3-island-size/EXISTING-EXTERNAL-EDGES.md; capability is not activation.
//
// `--reason dependency-graph` (#4218, schemaVersion 3) re-pins a reviewed move
// of the pnpm dependency graph from a fresh artifact measured at a checkout
// that already carries the new graph: the artifact's fingerprint must equal the
// checkout's, exactly `dependencyFingerprint.sha256` may move (policyVersion,
// parser and helperSha256 belong to `fingerprint-helper` / `fingerprint-policy`),
// and the totals, Cargo.lock, package version, fixtures, runners, toolchain,
// normalizer, metric and allowance must be unchanged. Only
// DEPENDENCY_GRAPH_MOVEMENT is written (assertAllowedMovement), and
// lock-fingerprint-inventory.json is regenerated from the checkout's `--explain`.
// It requires `--source-sha` (the artifact's measured source, which may be a PR
// merge source distinct from the probe `--head-sha`) and a saved
// measurement.normalized.json that agrees with the saved bytes.
//
// `--reason fingerprint-policy` (#4218, schemaVersion 3) is the policy-version
// counterpart: the four `dependencyFingerprint` keys may move only together
// with an exact one-step `policyVersion` increment that equals the checkout
// helper's exported FINGERPRINT_POLICY_VERSION, and `parser` / `helperSha256`
// must be the checkout helper's own (fingerprintPolicyRefusal below). Run
// provenance carries only {policyVersion, sha256}, so `parser` and
// `helperSha256` are proven against the checkout helper, never attested by the
// artifact. Every dependency-graph refusal applies unchanged, only
// FINGERPRINT_POLICY_MOVEMENT is written, and the policy identity line of
// LOCK-FINGERPRINT-POLICY.md is rewritten with the contract.
//
// Every fingerprint computation, the helper's exported identity and the
// `--explain` inventory come from the lock-fingerprint.mjs inside the selected
// checkout root (loadCheckoutHelper below); in the repository itself that is
// the same file as the static helper.
//
// `--stage <dir>` writes every output under <dir> instead, plus a
// stage-manifest.json binding each destination's live and staged SHA-256; the
// shared test, README and inventory files already staged there are the input,
// so the Darwin script can stage into the same directory. `--apply <dir>`
// (either script) writes every staged file once all live and staged hashes
// still match and a paired reason is staged for both platforms
// (openOutputSession / applyStage below). The paired reasons
// (`dependency-graph`, `fingerprint-policy`) refuse to run without `--stage`.
// A stage directory holds one reason. Each paired run records its transition
// identity (pairedTransition) and measured source; staging the second platform
// and `--apply` refuse unless both transitions are equal and Darwin measured
// the Linux merge source or its probe head (pairedRunsProblem below). A failed
// staging write leaves a partial stage that both `--apply` and `--stage`
// refuse; the error says to discard the directory and stage again.
//
// No file is written before every validation and every rendered output
// succeeds; `--apply` refuses if any destination moved since staging, writes
// every destination to a temporary sibling first and only then renames them
// into place, restoring already-renamed files if a rename fails. There is no
// force or ignore-provenance option of any kind.
// Optional --reviewed-cargo-transition recognizes only the separately reviewed
// #318 non-additive lock pair and its complete record inventory. It does not
// bypass artifact, platform, identity or unchanged-total validation.

import { execFileSync, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";

import { sameHashMaps } from "../research/v3-island-size/check-budget.mjs";
import {
  IDENTITY_NORMALIZATION_VERSION,
  IDENTITY_NORMALIZED_METRIC,
  NORMALIZED_SIDECAR,
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
  "fingerprint-helper": {
    outcome: "fingerprint-helper-repin",
    heading: "fingerprint-helper re-pin",
    requires: 3,
    accepts: [3],
  },
  "cargo-producer-lock": {
    outcome: "cargo-producer-lock-transition",
    heading: "cargo-producer-lock transition",
    requires: 3,
    accepts: [3],
  },
  "dependency-graph": {
    outcome: "dependency-graph-repin",
    heading: "dependency-graph re-pin",
    requires: 3,
    accepts: [3],
  },
  "fingerprint-policy": {
    outcome: "fingerprint-policy",
    heading: "fingerprint-policy re-pin",
    requires: 3,
    accepts: [3],
  },
  "cargo-internal-rewire": {
    outcome: "cargo-internal-rewire-transition",
    heading: "cargo-internal-rewire transition",
    requires: 3,
    accepts: [3],
  },
});

export const FINGERPRINT_HELPER_REASON = "fingerprint-helper";
export const CARGO_PRODUCER_LOCK_REASON = "cargo-producer-lock";
export const CARGO_INTERNAL_REWIRE_REASON = "cargo-internal-rewire";
// The only keys a cargo-internal-rewire transition may write; baselineEvidence
// is replaced as a whole and carries the `cargoRewire` transition record.
export const CARGO_INTERNAL_REWIRE_MOVEMENT = Object.freeze([
  "toolchain.cargoLockSha256",
  "outcome",
  "baselineSourceSha",
  "platformStatus",
  "baselineEvidence",
]);
export const DEPENDENCY_GRAPH_REASON = "dependency-graph";
// The only keys a dependency-graph re-pin may write; baselineEvidence is
// replaced as a whole.
export const DEPENDENCY_GRAPH_MOVEMENT = Object.freeze([
  "dependencyFingerprint.sha256",
  "toolchain.pnpmLockSha256",
  "outcome",
  "baselineSourceSha",
  "platformStatus",
  "baselineEvidence",
]);
export const FINGERPRINT_POLICY_REASON = "fingerprint-policy";
// The only keys a fingerprint-policy re-pin may write; baselineEvidence is
// replaced as a whole.
export const FINGERPRINT_POLICY_MOVEMENT = Object.freeze([
  "dependencyFingerprint.policyVersion",
  "dependencyFingerprint.parser",
  "dependencyFingerprint.sha256",
  "dependencyFingerprint.helperSha256",
  "toolchain.pnpmLockSha256",
  "outcome",
  "baselineSourceSha",
  "platformStatus",
  "baselineEvidence",
]);
// Reasons whose artifact must carry the operator's --source-sha and a saved
// normalizer sidecar that agrees with the saved bytes.
export const SOURCE_BOUND_REASONS = Object.freeze([
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_POLICY_REASON,
  CARGO_INTERNAL_REWIRE_REASON,
]);
// Reasons that are applied only once both platforms are staged.
export const PAIRED_REASONS = Object.freeze([
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_POLICY_REASON,
  CARGO_INTERNAL_REWIRE_REASON,
]);
// Reasons that regenerate lock-fingerprint-inventory.json from the checkout helper.
export const INVENTORY_REASONS = Object.freeze([
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_POLICY_REASON,
]);
// Digest of the checked-in, separately approved decision, including its graph
// identities and rationale. A caller-supplied JSON is evidence, not authority.
const REVIEWED_CARGO_TRANSITION_SHA256 =
  "e4b89f02bea8ed6eace98023c08561ff9de2289a65d7fd5b14e0e3d6c1d008ed";

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

export const FINGERPRINT_HELPER_FILE = SIZE_DIR + "/lock-fingerprint.mjs";

// The lock-fingerprint.mjs module of the checkout at `root`, imported from that
// file (keyed by its bytes, so an edited helper is never served from cache).
// Every fingerprint, exported identity and inventory a re-pin relies on comes
// from this module, never from the copy next to this script.
export async function loadCheckoutHelper(root = ".") {
  const path = resolve(root, FINGERPRINT_HELPER_FILE);
  let bytes;
  try {
    bytes = readFileSync(path);
  } catch (error) {
    throw new Error(
      "cannot read the checkout's " + FINGERPRINT_HELPER_FILE + ": " + error.message,
      {
        cause: error,
      },
    );
  }
  return import(pathToFileURL(path).href + "?sha256=" + sha256(bytes));
}

// What a loaded helper exports as its own identity, in contract key order.
export function helperIdentity(helper) {
  return {
    policyVersion: helper.FINGERPRINT_POLICY_VERSION,
    parser: helper.FINGERPRINT_PARSER,
    helperSha256: helper.fingerprintHelperSha256(),
  };
}

// The schemaVersion-3 `dependencyFingerprint` block a loaded helper computes,
// in contract key order. Throws (fail-closed) when it rejects the lock.
export function checkoutFingerprint(helper, lockText, currentVersion, repoRoot) {
  const fingerprint = helper.computeLockFingerprint({
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

// The checkout's identity, read from `root` with `helper`, the checkout's own
// lock-fingerprint.mjs (loadCheckoutHelper). A fingerprint failure is kept as
// dependencyFingerprintError and reported by artifactFingerprintProblem.
export function readCheckoutIdentity(contract, root, helper) {
  if (!helper || typeof helper.computeLockFingerprint !== "function") {
    throw new Error("readCheckoutIdentity needs the checkout's loaded lock-fingerprint.mjs");
  }
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
    checkoutHelper: helperIdentity(helper),
  };
  try {
    checkout.dependencyFingerprint = checkoutFingerprint(
      helper,
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
        " differ from the contract); " +
        FINGERPRINT_MOVE_REMEDY +
        ", not a " +
        reason +
        " re-pin",
    );
  }
}

// Shared by both platforms' refusal of a fingerprint move under another reason.
export const FINGERPRINT_MOVE_REMEDY =
  "a reachable dependency change needs `--reason dependency-graph` (#4218) and a policy or parser change needs `fingerprint-policy` (#4218)";

function dependencyGraphError(message) {
  return new Error("--reason " + DEPENDENCY_GRAPH_REASON + ": " + message);
}

// Why the contract's fingerprint cannot be re-pinned to `local` under
// dependency-graph, or null: exactly `sha256` must move.
export function dependencyGraphFingerprintRefusal(contract, local) {
  const moved = contractFingerprintMoves(contract, local);
  const other = moved.filter((key) => key !== "sha256");
  if (other.length > 0) {
    return (
      "--reason " +
      DEPENDENCY_GRAPH_REASON +
      " moves only dependencyFingerprint.sha256, but " +
      other.join(", ") +
      " differ from the contract; a helper-only edit needs `fingerprint-helper` (#4198) and a policy or parser change needs `fingerprint-policy` (#4218)"
    );
  }
  if (!moved.includes("sha256")) {
    return (
      "--reason " +
      DEPENDENCY_GRAPH_REASON +
      ": the contract already pins this fingerprint; nothing to re-pin"
    );
  }
  return null;
}

// The contract identity a dependency-graph re-pin must leave untouched, as the
// names that differ from `measured` ({packageVersion, normalizerSha256,
// fixtureSha256, runnerSha256, toolchain?}); metric, normalization version and
// allowance must be the ones this checkout measures.
export function dependencyGraphIdentityMoves(contract, measured) {
  const moved = [];
  if (measured.packageVersion !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (measured.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  if (!sameHashMaps(measured.fixtureSha256, contract.fixtureSha256)) moved.push("fixtureSha256");
  if (!sameHashMaps(measured.runnerSha256, contract.runnerSha256)) moved.push("runnerSha256");
  if (measured.toolchain !== undefined) {
    for (const key of IDENTITY_TOOLCHAIN_KEYS) {
      if (measured.toolchain?.[key] !== contract.toolchain[key]) moved.push("toolchain." + key);
    }
  }
  if (contract.metric !== IDENTITY_NORMALIZED_METRIC) moved.push("metric");
  if (contract.normalizationVersion !== IDENTITY_NORMALIZATION_VERSION) {
    moved.push("normalizationVersion");
  }
  if (!isDeepStrictEqual(contract.allowance, { raw: 0, gzipNormalized: 0 })) {
    moved.push("allowance");
  }
  return moved;
}

// What each fingerprint re-pin reason may move, for its identity refusal.
const FINGERPRINT_REASON_SCOPE = {
  [DEPENDENCY_GRAPH_REASON]: "moves only dependencyFingerprint.sha256",
  [FINGERPRINT_POLICY_REASON]:
    "moves only the dependency fingerprint, together with a policyVersion step",
};

export function dependencyGraphIdentityRefusal(moved, reason = DEPENDENCY_GRAPH_REASON) {
  return (
    "--reason " +
    reason +
    " " +
    FINGERPRINT_REASON_SCOPE[reason] +
    ", but " +
    moved.join(", ") +
    " differ from the contract; use the reason that matches the change (a toolchain bump such as Hono needs its own reviewed procedure)"
  );
}

// dependency-graph on Linux: the artifact carries this checkout's fingerprint,
// only the contract's `sha256` moves, and every other reviewed identity input,
// Cargo.lock included, equals the contract.
function assertDependencyGraphMovement(contract, evidence) {
  const identity = evidence.identity;
  if (!identity) throw dependencyGraphError("needs the artifact's measured identity inputs");
  const problem = artifactFingerprintProblem(evidence.artifactFingerprint, evidence);
  if (problem) throw dependencyGraphError(problem);
  const refusal = dependencyGraphFingerprintRefusal(contract, evidence.dependencyFingerprint);
  if (refusal) throw new Error(refusal);
  assertCargoUnchanged(contract, evidence, DEPENDENCY_GRAPH_REASON);
  const moved = dependencyGraphIdentityMoves(contract, {
    packageVersion: evidence.version,
    normalizerSha256: evidence.normalizerSha256,
    fixtureSha256: identity.fixtureSha256,
    runnerSha256: identity.runnerSha256,
    toolchain: identity.toolchain,
  });
  if (moved.length > 0) throw new Error(dependencyGraphIdentityRefusal(moved));
}

// The fingerprint transition a dependency-graph re-pin records in baselineEvidence.
export function fingerprintTransitionRecord(contract, local) {
  return {
    policyVersion: contract.dependencyFingerprint.policyVersion,
    oldSha256: contract.dependencyFingerprint.sha256,
    newSha256: local.sha256,
  };
}

function fingerprintPolicyError(message) {
  return new Error("--reason " + FINGERPRINT_POLICY_REASON + ": " + message);
}

// Why the contract's fingerprint cannot move to the checkout's `local` under
// fingerprint-policy, or null. `helper` is the checkout helper's exported
// identity (helperIdentity): `policyVersion` must step up by exactly one to the
// helper's FINGERPRINT_POLICY_VERSION, and `parser` and `helperSha256` must be
// the helper's own. A parser or helper move without a version step is refused.
export function fingerprintPolicyRefusal(contract, local, helper) {
  const pinned = contract.dependencyFingerprint;
  const versions = [pinned?.policyVersion, local?.policyVersion, helper?.policyVersion];
  if (!versions.every((version) => Number.isSafeInteger(version))) {
    return (
      "policyVersion must be an integer in the contract, the checkout fingerprint and the helper's exported FINGERPRINT_POLICY_VERSION, found " +
      versions.map(String).join(", ")
    );
  }
  const step = local.policyVersion - pinned.policyVersion;
  if (step === 0) {
    if (local.parser !== pinned.parser) {
      return (
        "parser moved from " +
        pinned.parser +
        " to " +
        local.parser +
        " without a policyVersion step; a parser change is a policyVersion bump"
      );
    }
    if (local.helperSha256 !== pinned.helperSha256 && local.sha256 === pinned.sha256) {
      return "helperSha256 moved without a policyVersion step; a helper-only edit needs `fingerprint-helper` (#4198)";
    }
    return (
      "policyVersion did not move; use fingerprint-helper or dependency-graph (the contract and the checkout are both at policy v" +
      pinned.policyVersion +
      ")"
    );
  }
  if (step !== 1) {
    return (
      "policyVersion must increase by exactly one, but the contract pins v" +
      pinned.policyVersion +
      " and the checkout computes v" +
      local.policyVersion
    );
  }
  if (local.policyVersion !== helper.policyVersion) {
    return (
      "the checkout's policyVersion " +
      local.policyVersion +
      " does not match the helper's exported FINGERPRINT_POLICY_VERSION " +
      helper.policyVersion
    );
  }
  if (local.parser !== helper.parser) {
    return (
      "the checkout's parser " +
      local.parser +
      " does not match the helper's exported FINGERPRINT_PARSER " +
      helper.parser
    );
  }
  if (local.helperSha256 !== helper.helperSha256) {
    return "the checkout's helperSha256 does not match the checkout helper's fingerprintHelperSha256()";
  }
  return null;
}

// The fingerprint transition a fingerprint-policy re-pin records in baselineEvidence.
export function fingerprintPolicyTransitionRecord(contract, local) {
  return {
    from: contractFingerprint(contract.dependencyFingerprint),
    to: contractFingerprint(local),
  };
}

function policyText(fingerprint) {
  return (
    "policy v" +
    fingerprint.policyVersion +
    " (" +
    fingerprint.parser +
    ", sha256 " +
    fingerprint.sha256 +
    ", helper " +
    fingerprint.helperSha256 +
    ")"
  );
}

export function fingerprintPolicyTransitionText(transition) {
  return (
    "dependency fingerprint " + policyText(transition.from) + " to " + policyText(transition.to)
  );
}

function policyMarkdown(fingerprint) {
  return (
    "policy v" +
    fingerprint.policyVersion +
    " (`" +
    fingerprint.parser +
    "`, `" +
    fingerprint.sha256 +
    "`, helper `" +
    fingerprint.helperSha256 +
    "`)"
  );
}

export function fingerprintPolicyTransitionMarkdown(transition) {
  return (
    "the dependency fingerprint moved from " +
    policyMarkdown(transition.from) +
    " to " +
    policyMarkdown(transition.to) +
    " ([LOCK-FINGERPRINT-POLICY.md](LOCK-FINGERPRINT-POLICY.md), whose policy identity line moved with it). `policyVersion` and `sha256` equal the artifact's run provenance; `parser` and `helperSha256` are proven against the checkout's own `lock-fingerprint.mjs`, which the artifact does not attest. `lock-fingerprint-inventory.json` was regenerated from that helper's `--explain`"
  );
}

// fingerprint-policy on Linux: the artifact carries this checkout's
// fingerprint, the contract's fingerprint moves only with a one-step
// policyVersion increment matching the checkout helper, and every other
// reviewed identity input, Cargo.lock included, equals the contract.
function assertFingerprintPolicyMovement(contract, evidence) {
  const identity = evidence.identity;
  if (!identity) throw fingerprintPolicyError("needs the artifact's measured identity inputs");
  const problem = artifactFingerprintProblem(evidence.artifactFingerprint, evidence);
  if (problem) throw fingerprintPolicyError(problem);
  if (!isRecord(evidence.checkoutHelper)) {
    throw fingerprintPolicyError("needs the checkout helper's exported identity");
  }
  const refusal = fingerprintPolicyRefusal(
    contract,
    evidence.dependencyFingerprint,
    evidence.checkoutHelper,
  );
  if (refusal) throw fingerprintPolicyError(refusal);
  assertCargoUnchanged(contract, evidence, FINGERPRINT_POLICY_REASON);
  const moved = dependencyGraphIdentityMoves(contract, {
    packageVersion: evidence.version,
    normalizerSha256: evidence.normalizerSha256,
    fixtureSha256: identity.fixtureSha256,
    runnerSha256: identity.runnerSha256,
    toolchain: identity.toolchain,
  });
  if (moved.length > 0) {
    throw new Error(dependencyGraphIdentityRefusal(moved, FINGERPRINT_POLICY_REASON));
  }
}

const POLICY_DOC_NAME = "LOCK-FINGERPRINT-POLICY.md";
// Any line opening with the marker counts, so a malformed duplicate is refused
// rather than silently left behind.
const POLICY_IDENTITY_MARKER = /^Policy identity:/gm;
// The parser is written as a JSON string literal, escapes included, so the
// pattern accepts exactly what policyIdentityLine() renders.
const POLICY_IDENTITY_LINE =
  /^Policy identity: `policyVersion: \d+`, `parser: "(?:[^"\\\n]|\\.)*"`/m;

function policyIdentityLine(fingerprint) {
  return (
    "Policy identity: `policyVersion: " +
    fingerprint.policyVersion +
    "`, `parser: " +
    JSON.stringify(fingerprint.parser) +
    "`"
  );
}

// Rewrites the policy identity line of LOCK-FINGERPRINT-POLICY.md to the
// re-pinned `fingerprint`; refuses a document without exactly one such line.
export function rewritePolicyIdentity(source, fingerprint) {
  const markers = source.match(POLICY_IDENTITY_MARKER)?.length ?? 0;
  if (markers > 1) {
    throw fingerprintPolicyError(
      markers + " policy identity lines in " + POLICY_DOC_NAME + "; expected exactly one",
    );
  }
  if (markers === 0 || !POLICY_IDENTITY_LINE.test(source)) {
    throw fingerprintPolicyError("policy identity line not found in " + POLICY_DOC_NAME);
  }
  const line = policyIdentityLine(fingerprint);
  // A replacer function: `$` in the parser must not be read as a pattern.
  return source.replace(POLICY_IDENTITY_LINE, () => line);
}

// Refuses a rewrite of `before` into `after` that moved any key outside
// `allowlist`. Entries match exactly; an entry ending in `.`, and
// `baselineEvidence` (which is replaced as a whole), also match by prefix.
export function assertAllowedMovement(reason, allowlist, before, after) {
  const moved = contractDiffKeys(before, after).filter(
    (key) =>
      !allowlist.some((allowed) =>
        allowed.endsWith(".")
          ? key.startsWith(allowed)
          : key === allowed || (allowed === "baselineEvidence" && key.startsWith(allowed + ".")),
      ),
  );
  if (moved.length > 0) {
    throw new Error("--reason " + reason + ": would move " + moved.join(", ") + "; refusing");
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

// release-bump / runtime-identity never move Cargo.lock: the measured artifact
// (which equals the checkout) must carry the contract's Cargo hash.
export function cargoMoveRefusal(reason, measuredCargoSha, contract) {
  if (measuredCargoSha === contract.toolchain.cargoLockSha256) return null;
  return (
    "--reason " +
    reason +
    " cannot move Cargo.lock (the artifact and checkout carry " +
    String(measuredCargoSha) +
    ", the contract pins " +
    contract.toolchain.cargoLockSha256 +
    "); a reviewed additive producer-lock change needs --reason " +
    CARGO_PRODUCER_LOCK_REASON +
    " first"
  );
}

function assertCargoUnchanged(contract, evidence, reason) {
  if (!evidence.identity) {
    throw new Error("--reason " + reason + " needs the artifact's measured identity inputs");
  }
  const refusal = cargoMoveRefusal(reason, evidence.identity.cargoLockSha256, contract);
  if (refusal) throw new Error(refusal);
}

export const CARGO_ARGS = Object.freeze([
  "expected-old-cargo-sha",
  "expected-new-cargo-sha",
  "old-cargo-ref",
  "old-cargo-lock",
  "added-packages",
  "reviewed-cargo-transition",
]);
// The Cargo arguments both lock-moving reasons share; `--added-packages` is
// cargo-producer-lock's alone and REWIRE_ARGS are cargo-internal-rewire's.
export const SHARED_CARGO_ARGS = Object.freeze([
  "expected-old-cargo-sha",
  "expected-new-cargo-sha",
  "old-cargo-ref",
  "old-cargo-lock",
]);
export const REWIRE_ARGS = Object.freeze([
  "rewire-declaration",
  "old-closure",
  "new-closure",
  "new-cargo-ref",
]);
// Corroboration only: the old lock's source of truth is `--old-cargo-ref`.
export const OPTIONAL_CARGO_ARGS = Object.freeze(["old-cargo-lock", "reviewed-cargo-transition"]);
// Optional: cites the commit storing the new lock; the checkout's bytes are the input.
export const OPTIONAL_REWIRE_ARGS = Object.freeze(["new-cargo-ref"]);
export const OLD_CARGO_LOCK_PATH = "Cargo.lock";
const LOCK_MOVING_REASONS = Object.freeze([
  CARGO_PRODUCER_LOCK_REASON,
  CARGO_INTERNAL_REWIRE_REASON,
]);
// The only keys a cargo-producer-lock transition may write; baselineEvidence is
// replaced as a whole.
export const CARGO_PRODUCER_LOCK_MOVEMENT = Object.freeze([
  "toolchain.cargoLockSha256",
  "outcome",
  "baselineSourceSha",
  "platformStatus",
  "baselineEvidence",
]);

function cargoError(message) {
  return new Error("--reason " + CARGO_PRODUCER_LOCK_REASON + ": " + message);
}

// `name@version,...` -> the allowlist, refusing an empty list, a malformed
// entry or a repeated identity.
export function parseAddedPackages(text) {
  const entries = String(text ?? "")
    .split(",")
    .map((entry) => entry.trim());
  if (entries.length === 0 || entries.some((entry) => !/^[^@\s]+@[^@\s]+$/.test(entry))) {
    throw cargoError("--added-packages must be a comma-separated list of name@version");
  }
  const repeated = entries.filter((entry, i) => entries.indexOf(entry) !== i);
  if (repeated.length > 0) {
    throw cargoError("--added-packages repeats " + [...new Set(repeated)].join(", "));
  }
  return entries;
}

// `values` are the parsed `--key value` pairs; the Cargo arguments belong to
// the two lock-moving reasons: `--added-packages` to cargo-producer-lock alone
// (which requires all but OPTIONAL_CARGO_ARGS), the rest to both.
export function checkCargoProducerLockArgs(values) {
  if (!LOCK_MOVING_REASONS.includes(values.reason)) {
    const stray = CARGO_ARGS.filter((key) => Object.hasOwn(values, key));
    if (stray.length > 0) {
      throw new Error(
        "--" +
          stray[0] +
          " is only for --reason " +
          (SHARED_CARGO_ARGS.includes(stray[0])
            ? LOCK_MOVING_REASONS.join(", ")
            : CARGO_PRODUCER_LOCK_REASON),
      );
    }
    return;
  }
  if (values.reason !== CARGO_PRODUCER_LOCK_REASON) {
    for (const key of ["added-packages", "reviewed-cargo-transition"]) {
      if (Object.hasOwn(values, key)) {
        throw new Error("--" + key + " is only for --reason " + CARGO_PRODUCER_LOCK_REASON);
      }
    }
    return;
  }
  for (const key of CARGO_ARGS) {
    if (!OPTIONAL_CARGO_ARGS.includes(key) && !values[key]) {
      throw new Error("required --" + key);
    }
  }
  checkSharedCargoArgs(values);
  parseAddedPackages(values["added-packages"]);
}

function checkSharedCargoArgs(values) {
  for (const key of ["expected-old-cargo-sha", "expected-new-cargo-sha"]) {
    if (!/^[0-9a-f]{64}$/.test(values[key])) {
      throw new Error("--" + key + " must be a 64-character SHA-256");
    }
  }
  parseOldCargoRef(values["old-cargo-ref"]);
}

// cargo-internal-rewire takes the shared Cargo arguments plus REWIRE_ARGS
// (all but OPTIONAL_REWIRE_ARGS required); REWIRE_ARGS belong to it alone.
export function checkCargoInternalRewireArgs(values) {
  if (values.reason !== CARGO_INTERNAL_REWIRE_REASON) {
    const stray = REWIRE_ARGS.filter((key) => Object.hasOwn(values, key));
    if (stray.length > 0) {
      throw new Error("--" + stray[0] + " is only for --reason " + CARGO_INTERNAL_REWIRE_REASON);
    }
    return;
  }
  for (const key of [...SHARED_CARGO_ARGS, ...REWIRE_ARGS]) {
    if (OPTIONAL_CARGO_ARGS.includes(key) || OPTIONAL_REWIRE_ARGS.includes(key)) continue;
    if (!values[key]) throw new Error("required --" + key);
  }
  checkSharedCargoArgs(values);
  if (Object.hasOwn(values, "new-cargo-ref")) parseNewCargoRef(values["new-cargo-ref"]);
}

// `<ref>[:<path>]` for the commit that stores the new Cargo.lock bytes.
export function parseNewCargoRef(text) {
  try {
    return parseRefSpec(text, OLD_CARGO_LOCK_PATH, "--new-cargo-ref");
  } catch (error) {
    throw rewireError(error.message);
  }
}

// `<ref>[:<path>]` -> {ref, path}, the path defaulting to Cargo.lock.
export function parseOldCargoRef(text) {
  try {
    return parseRefSpec(text, OLD_CARGO_LOCK_PATH, "--old-cargo-ref");
  } catch (error) {
    throw cargoError(error.message);
  }
}

// The artifact reasons that record a flat `repinnedFrom` predecessor; the two
// migrations do not (fingerprint-migration keeps `migratedFrom`). Only these
// accept --previous-contract-ref.
export const REPINNED_FROM_REASONS = Object.freeze([
  "release-bump",
  "runtime-identity",
  "lockfile-provenance",
  CARGO_PRODUCER_LOCK_REASON,
  DEPENDENCY_GRAPH_REASON,
  FINGERPRINT_POLICY_REASON,
  CARGO_INTERNAL_REWIRE_REASON,
]);

export function checkPreviousContractRefArgs(values, reasons = REPINNED_FROM_REASONS) {
  if (!Object.hasOwn(values, "previous-contract-ref")) return;
  if (!reasons.includes(values.reason)) {
    throw new Error("--previous-contract-ref is only for --reason " + reasons.join(", "));
  }
  checkGitRef(values["previous-contract-ref"], "--previous-contract-ref");
}

// Splits a Cargo.lock into its header, its `[[package]]` records and any other
// section, each record without its trailing blank line. The split is lossless,
// so a layout it cannot reassemble byte-for-byte is refused.
export function parseCargoLock(text, label) {
  if (typeof text !== "string" || !text.endsWith("\n")) {
    throw cargoError(label + " is empty or does not end with a newline");
  }
  const sections = text.split(/(?=^\[)/m);
  const header = sections[0].startsWith("[") ? "" : sections.shift();
  const packages = [];
  const other = [];
  sections.forEach((section, i) => {
    const body = section.replace(/\n+$/, "\n");
    if (section !== (i === sections.length - 1 ? body : body + "\n")) {
      throw cargoError(
        label + " has an unexpected record separator near " + JSON.stringify(body.slice(0, 60)),
      );
    }
    if (!body.startsWith("[[package]]\n")) {
      other.push(body);
      return;
    }
    const name = /^name = "([^"]+)"$/m.exec(body)?.[1];
    const version = /^version = "([^"]+)"$/m.exec(body)?.[1];
    if (!name || !version) {
      throw cargoError(
        label + " has a [[package]] record without name and version: " + JSON.stringify(body),
      );
    }
    packages.push({ id: name + "@" + version, body });
  });
  return { header, packages, other };
}

function countBodies(packages) {
  const counts = new Map();
  for (const { body } of packages) counts.set(body, (counts.get(body) ?? 0) + 1);
  return counts;
}

// The record-level diff of two Cargo.lock texts: `added` are new records whose
// bytes no old record had, `changed` old identities whose bytes moved, and
// `removed` old identities that are gone. Header and other sections must match.
export function diffCargoLocks(oldText, newText) {
  const before = parseCargoLock(oldText, "old Cargo.lock");
  const after = parseCargoLock(newText, "new Cargo.lock");
  const problems = [];
  if (before.header !== after.header) problems.push("the header differs");
  if (!isDeepStrictEqual(before.other, after.other)) {
    problems.push("a non-package section differs");
  }
  const oldBodies = countBodies(before.packages);
  const added = [];
  for (const record of after.packages) {
    const left = oldBodies.get(record.body) ?? 0;
    if (left > 0) oldBodies.set(record.body, left - 1);
    else added.push(record.id);
  }
  const newIds = new Set(after.packages.map(({ id }) => id));
  const changed = [];
  const removed = [];
  for (const record of before.packages) {
    const left = oldBodies.get(record.body) ?? 0;
    if (left === 0) continue;
    oldBodies.set(record.body, left - 1);
    (newIds.has(record.id) ? changed : removed).push(record.id);
  }
  const oldIds = new Set(before.packages.map(({ id }) => id));
  const duplicates = added.filter(
    (id, i) => added.indexOf(id) !== i || (oldIds.has(id) && !changed.includes(id)),
  );
  return { problems, added, changed, removed, duplicates: [...new Set(duplicates)] };
}

function decodeLock(bytes) {
  return typeof bytes === "string" ? bytes : Buffer.from(bytes ?? "").toString("utf8");
}

// Optional, separately reviewed non-additive decision. This is an exact byte
// transition, never a wildcard exception for changed records. The normal proof
// remains additive-only, and neither proof admits measurements on its own.
export function proveReviewedCargoTransition(reviewText, oldText, newText) {
  let review;
  try {
    review = JSON.parse(reviewText);
  } catch {
    throw cargoError("invalid reviewed Cargo transition JSON");
  }
  const keys = (value, expected) =>
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    isDeepStrictEqual(Object.keys(value).sort(), [...expected].sort());
  if (
    !keys(review, ["schemaVersion", "id", "oldSha", "newSha", "delta", "review"]) ||
    review.schemaVersion !== 1 ||
    review.id !== "rolldown-native-318" ||
    review.oldSha !== "2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e" ||
    review.newSha !== "13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37"
  ) {
    throw cargoError("unsupported reviewed Cargo transition schema or identity");
  }
  if (review.oldSha !== sha256(oldText) || review.newSha !== sha256(newText)) {
    throw cargoError("reviewed Cargo transition old/new hashes do not match the exact lock bytes");
  }
  const before = parseCargoLock(oldText, "old Cargo.lock");
  const after = parseCargoLock(newText, "new Cargo.lock");
  const beforeMap = new Map(before.packages.map((record) => [record.id, record.body]));
  const afterMap = new Map(after.packages.map((record) => [record.id, record.body]));
  if (
    beforeMap.size !== before.packages.length ||
    afterMap.size !== after.packages.length ||
    before.header !== after.header ||
    !isDeepStrictEqual(before.other, after.other)
  ) {
    throw cargoError(
      "reviewed Cargo transition has duplicate identities or changed header/sections",
    );
  }
  const delta = { added: [], changed: [], removed: [] };
  for (const id of [...new Set([...beforeMap.keys(), ...afterMap.keys()])].sort()) {
    const old = beforeMap.get(id);
    const next = afterMap.get(id);
    if (old === next) continue;
    const row = {
      id,
      beforeSha256: old ? sha256(old) : null,
      afterSha256: next ? sha256(next) : null,
    };
    delta[!old ? "added" : !next ? "removed" : "changed"].push(row);
  }
  if (!isDeepStrictEqual(review.delta, delta)) {
    throw cargoError(
      "reviewed Cargo transition inventory differs from the exact added/changed/removed records",
    );
  }
  const detail = review.review;
  if (
    !keys(detail, [
      "sourceSha",
      "decision",
      "sharedReplacements",
      "windowsEdges",
      "featureGraphs",
      "nativeMeasurement",
    ]) ||
    !/^[0-9a-f]{40}$/.test(detail.sourceSha) ||
    typeof detail.decision !== "string" ||
    !detail.decision ||
    detail.nativeMeasurement !== "required-separately-for-each-platform"
  ) {
    throw cargoError("reviewed Cargo transition is missing its review or native measurement gate");
  }
  for (const [name, count, rowKeys] of [
    ["sharedReplacements", 14, ["name", "from", "to", "minimum", "reason"]],
    ["windowsEdges", 9, ["id", "from", "to", "reason"]],
  ]) {
    const rows = detail[name];
    if (
      !Array.isArray(rows) ||
      rows.length !== count ||
      rows.some(
        (row) =>
          !keys(row, rowKeys) || rowKeys.some((key) => typeof row[key] !== "string" || !row[key]),
      ) ||
      new Set(rows.map((row) => row.name ?? row.id)).size !== count
    ) {
      throw cargoError("reviewed Cargo transition needs the complete " + name + " review");
    }
  }
  for (const replacement of detail.sharedReplacements) {
    if (
      !delta.removed.some((row) => row.id === replacement.name + "@" + replacement.from) ||
      !delta.added.some((row) => row.id === replacement.name + "@" + replacement.to)
    ) {
      throw cargoError("reviewed shared replacement is not in the lock delta: " + replacement.name);
    }
  }
  for (const edge of detail.windowsEdges) {
    if (
      !delta.changed.some((row) => row.id === edge.id) ||
      !edge.from.startsWith("windows-sys ") ||
      !edge.to.startsWith("windows-sys ") ||
      !beforeMap.get(edge.id)?.includes(' "' + edge.from + '",') ||
      !afterMap.get(edge.id)?.includes(' "' + edge.to + '",')
    ) {
      throw cargoError("reviewed Windows edge is not in the lock delta: " + edge.id);
    }
  }
  const graphs = detail.featureGraphs;
  const graphCommands = {
    default: "cargo tree --locked -p zfb -e normal,build,features",
    native: "cargo tree --locked -p zfb --features rolldown-prototype -e normal,build,features",
    "no-v8": "cargo tree --locked -p zfb --no-default-features -e normal,build,features",
  };
  if (
    !Array.isArray(graphs) ||
    !isDeepStrictEqual(graphs.map((graph) => graph.mode).sort(), ["default", "native", "no-v8"]) ||
    graphs.some(
      (graph) =>
        !keys(graph, ["mode", "command", "sourceSha", "platform", "sha256"]) ||
        graph.sourceSha !== detail.sourceSha ||
        graph.platform !== "linux-x64" ||
        graph.command !== graphCommands[graph.mode] ||
        !/^[0-9a-f]{64}$/.test(graph.sha256),
    )
  ) {
    throw cargoError(
      "reviewed Cargo transition needs all three identified producer feature graphs",
    );
  }
  if (sha256(reviewText) !== REVIEWED_CARGO_TRANSITION_SHA256) {
    throw cargoError(
      "reviewed Cargo transition manifest digest differs from the approved decision",
    );
  }
  return {
    id: review.id,
    manifestSha256: sha256(reviewText),
    sourceSha: detail.sourceSha,
    added: delta.added.map((row) => row.id),
    changed: delta.changed.map((row) => row.id),
    removed: delta.removed.map((row) => row.id),
  };
}

// Proves an additive or explicitly reviewed exact Cargo.lock transition and returns the
// `cargoTransition` evidence, or throws naming every failed check. `contractSha`
// is the pinned hash, `artifactSha` the measured run's, `newText` the checkout's
// Cargo.lock and `oldText` the old lock's git blob; both are hashed as raw
// bytes before decoding. `oldLockSource` is the blob's verified
// {commit, path, sha256} (readCargoLockEvidence); a filesystem path is refused.
export function proveCargoProducerLock({
  contractSha,
  artifactSha,
  expectedOldSha,
  expectedNewSha,
  oldText,
  newText,
  added,
  oldLockSource,
  reviewedTransitionText,
}) {
  const problems = [];
  if (expectedOldSha === expectedNewSha) {
    problems.push("--expected-old-cargo-sha equals --expected-new-cargo-sha; no Cargo.lock move");
  }
  if (contractSha !== expectedOldSha) {
    problems.push("the contract pins Cargo.lock " + contractSha + ", not --expected-old-cargo-sha");
  }
  const oldSha = sha256(oldText ?? "");
  const source = isRecord(oldLockSource)
    ? {
        commit: oldLockSource.commit,
        path: oldLockSource.path,
        sha256: oldLockSource.sha256,
      }
    : null;
  if (
    !source ||
    !/^[0-9a-f]{40}$/.test(source.commit ?? "") ||
    typeof source.path !== "string" ||
    source.path === "" ||
    source.sha256 !== oldSha
  ) {
    problems.push(
      "the old Cargo.lock source must be its verified git blob {commit, path, sha256}, not " +
        JSON.stringify(oldLockSource),
    );
  }
  if (oldSha !== expectedOldSha) {
    problems.push(
      (source ? source.commit + ":" + source.path : String(oldLockSource)) +
        " hashes to " +
        oldSha +
        ", not --expected-old-cargo-sha",
    );
  }
  const newSha = sha256(newText ?? "");
  if (newSha !== expectedNewSha) {
    problems.push(
      "the checkout's Cargo.lock hashes to " + newSha + ", not --expected-new-cargo-sha",
    );
  }
  if (artifactSha !== expectedNewSha) {
    problems.push(
      "the artifact measured Cargo.lock " + String(artifactSha) + ", not --expected-new-cargo-sha",
    );
  }
  if (problems.length > 0) throw cargoError(problems.join("; "));
  if (reviewedTransitionText !== undefined) {
    const proof = proveReviewedCargoTransition(
      reviewedTransitionText,
      decodeLock(oldText),
      decodeLock(newText),
    );
    if (!isDeepStrictEqual([...added].sort(), proof.added)) {
      throw cargoError("--added-packages differs from the reviewed new identities");
    }
    const { id, manifestSha256, sourceSha, ...delta } = proof;
    return {
      oldSha,
      newSha,
      ...delta,
      oldLockSource: source,
      review: { id, manifestSha256, sourceSha },
    };
  }
  const diff = diffCargoLocks(decodeLock(oldText), decodeLock(newText));
  const allowlist = [...added].sort();
  const found = [...diff.added].sort();
  if (diff.changed.length > 0) {
    problems.push(
      "changed records " +
        diff.changed.join(", ") +
        " (a reviewed internal dependency rewiring needs --reason " +
        CARGO_INTERNAL_REWIRE_REASON +
        ")",
    );
  }
  if (diff.removed.length > 0) problems.push("removed records " + diff.removed.join(", "));
  if (diff.duplicates.length > 0) {
    problems.push("duplicate package identities " + diff.duplicates.join(", "));
  }
  problems.push(...diff.problems);
  const unexpected = found.filter((id) => !allowlist.includes(id));
  const missing = allowlist.filter((id) => !found.includes(id));
  if (unexpected.length > 0) problems.push("unexpected additions " + unexpected.join(", "));
  if (missing.length > 0) problems.push("allowlisted but not added " + missing.join(", "));
  if (problems.length > 0) {
    throw cargoError(
      "the Cargo.lock move is not the reviewed additive transition (" + problems.join("; ") + ")",
    );
  }
  return {
    oldSha: expectedOldSha,
    newSha: expectedNewSha,
    added: found,
    changed: [],
    removed: [],
    oldLockSource: source,
  };
}

// Refuses a cargo-producer-lock rewrite that moved any key outside
// CARGO_PRODUCER_LOCK_MOVEMENT.
export function assertCargoProducerLockMovement(before, after) {
  assertAllowedMovement(CARGO_PRODUCER_LOCK_REASON, CARGO_PRODUCER_LOCK_MOVEMENT, before, after);
}

// cargo-producer-lock (and cargo-internal-rewire) may move only Cargo.lock:
// everything else the contract pins must equal the measured artifact and this
// checkout.
function assertCargoOnlyMovement(contract, evidence, reason = CARGO_PRODUCER_LOCK_REASON) {
  const fail = (message) => new Error("--reason " + reason + ": " + message);
  const identity = evidence.identity;
  if (!identity) throw fail("needs the artifact's measured identity inputs");
  const moved = [];
  if (evidence.version !== contract.toolchain.packageVersion) moved.push("packageVersion");
  if (evidence.normalizerSha256 !== contract.normalizerSha256) moved.push("normalizerSha256");
  if (evidence.pnpmLockSha256 !== contract.toolchain.pnpmLockSha256) moved.push("pnpmLockSha256");
  if (!sameHashMaps(identity.fixtureSha256, contract.fixtureSha256)) moved.push("fixtureSha256");
  if (!sameHashMaps(identity.runnerSha256, contract.runnerSha256)) moved.push("runnerSha256");
  for (const key of IDENTITY_TOOLCHAIN_KEYS) {
    if (identity.toolchain?.[key] !== contract.toolchain[key]) moved.push("toolchain." + key);
  }
  if (moved.length > 0) {
    throw fail(
      "allows only Cargo.lock to move, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change",
    );
  }
}

function rewireError(message) {
  return new Error("--reason " + CARGO_INTERNAL_REWIRE_REASON + ": " + message);
}

// The reviewed transition declaration `--rewire-declaration` points at. Every
// field is checked against the actual blobs; a declaration never admits a move
// by itself.
//
//   {
//     "schemaVersion": 1,
//     "producer": "zfb",                       // the measured CLI package
//     "oldCargoLockSha256": "<64 hex>",        // == --expected-old-cargo-sha
//     "newCargoLockSha256": "<64 hex>",        // == --expected-new-cargo-sha
//     "added": [{                              // new [[package]] records
//       "id": "zfb-js-host@0.0.0",
//       "manifest": "crates/zfb-js-host/Cargo.toml",
//       "sha256": "<64 hex of the exact record bytes>",
//       "dependencies": ["base64", "getrandom 0.3.4", ...]
//     }],
//     "changed": [{                            // rewired existing records
//       "id": "zfb-render@0.0.0",
//       "manifest": "crates/zfb-render/Cargo.toml",
//       "oldSha256": "<64 hex of the old record bytes>",
//       "newSha256": "<64 hex of the new record bytes>",
//       "removedDependencies": ["sha1", "sha2"],
//       "addedDependencies": ["zfb-js-host"]
//     }],
//     "closure": {                             // expected producer-closure rows
//       "added": ["zfb-js-host@0.0.0"],        // rows only in the new closure
//       "changed": []                          // rows whose features moved
//     }
//   }
//
// `added`/`changed` ids are `name@version`; a dependency entry is written as
// Cargo.lock writes it (`name`, `name version` or `name version (source)`).
export const REWIRE_DECLARATION_SCHEMA = 1;
// Explicit opt-in to adding edges to existing external records. This does not
// broaden schema 1's internal-only rewires or authorize a live transition.
export const EXISTING_EXTERNAL_EDGE_SCHEMA = 2;
// The measured CLI package: island sizes are produced by the `zfb` crate's
// binary, so a declaration and its closure receipts may name no other producer.
export const PRODUCER_PACKAGE = "zfb";

const LOCK_ID = /^[^@\s]+@[^@\s]+$/;
const DEPENDENCY_ENTRY = /^(\S+)(?: (\S+))?(?: \((.+)\))?$/;

function isStringArray(value) {
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

function sortedUnique(entries) {
  return [...new Set(entries)].sort();
}

// Structural check of a parsed declaration; returns the normalized declaration.
export function checkRewireDeclaration(declaration) {
  const fail = (message) => rewireError("--rewire-declaration " + message);
  if (!isRecord(declaration)) throw fail("must be a JSON object");
  const externalMode = declaration.schemaVersion === EXISTING_EXTERNAL_EDGE_SCHEMA;
  if (declaration.schemaVersion !== REWIRE_DECLARATION_SCHEMA && !externalMode) {
    throw fail("schemaVersion must be 1 or 2");
  }
  if (!externalMode && Object.hasOwn(declaration, "externalEdges")) {
    throw fail("externalEdges requires schemaVersion 2");
  }
  if (declaration.producer !== PRODUCER_PACKAGE) {
    throw fail(
      "producer must be " +
        PRODUCER_PACKAGE +
        ", the measured CLI package, not " +
        JSON.stringify(declaration.producer),
    );
  }
  for (const key of ["oldCargoLockSha256", "newCargoLockSha256"]) {
    if (!isSha(declaration[key])) throw fail(key + " must be a 64-character SHA-256");
  }
  if (declaration.oldCargoLockSha256 === declaration.newCargoLockSha256) {
    throw fail("oldCargoLockSha256 equals newCargoLockSha256; no Cargo.lock move");
  }
  const added = declaration.added ?? [];
  const changed = declaration.changed ?? [];
  if (!Array.isArray(added) || !Array.isArray(changed)) {
    throw fail("added and changed must be arrays");
  }
  if (added.length + changed.length === 0) throw fail("declares no added or changed record");
  const checkManifest = (entry, where) => {
    if (!isRecord(entry) || !LOCK_ID.test(entry.id ?? "")) {
      throw fail(where + " entries need an id of the form name@version");
    }
    try {
      checkGitPath(entry.manifest, where + " " + entry.id + " manifest");
    } catch (error) {
      throw fail(error.message);
    }
    if (basename(entry.manifest) !== "Cargo.toml") {
      throw fail(where + " " + entry.id + " manifest must be a Cargo.toml path");
    }
  };
  for (const entry of added) {
    checkManifest(entry, "added");
    if (!isSha(entry.sha256)) throw fail("added " + entry.id + " needs sha256");
    if (!isStringArray(entry.dependencies)) {
      throw fail("added " + entry.id + " needs a dependencies array");
    }
  }
  for (const entry of changed) {
    checkManifest(entry, "changed");
    for (const key of ["oldSha256", "newSha256"]) {
      if (!isSha(entry[key])) throw fail("changed " + entry.id + " needs " + key);
    }
    if (entry.oldSha256 === entry.newSha256) {
      throw fail("changed " + entry.id + " declares identical old and new bytes");
    }
    for (const key of ["removedDependencies", "addedDependencies"]) {
      if (!isStringArray(entry[key])) throw fail("changed " + entry.id + " needs " + key);
    }
    if (entry.removedDependencies.length + entry.addedDependencies.length === 0) {
      throw fail("changed " + entry.id + " declares no dependency-edge delta");
    }
  }
  const ids = [...added, ...changed].map((entry) => entry.id);
  const repeated = ids.filter((id, i) => ids.indexOf(id) !== i);
  if (repeated.length > 0) throw fail("repeats " + sortedUnique(repeated).join(", "));
  const closure = declaration.closure;
  if (!isRecord(closure) || !isStringArray(closure.added) || !isStringArray(closure.changed)) {
    throw fail("closure must carry added and changed id arrays");
  }
  for (const id of [...closure.added, ...closure.changed]) {
    if (!ids.includes(id)) throw fail("closure names " + id + ", which is not declared");
  }
  let externalEdges;
  if (externalMode) {
    if (added.length || closure.added.length || changed.some((e) => e.removedDependencies.length)) {
      throw fail("schemaVersion 2 permits only added external edges on existing internal records");
    }
    externalEdges = declaration.externalEdges;
    if (!Array.isArray(externalEdges) || externalEdges.length === 0) {
      throw fail("schemaVersion 2 needs a nonempty externalEdges array");
    }
    const seen = new Set();
    for (const edge of externalEdges) {
      if (
        !isRecord(edge) ||
        !isDeepStrictEqual(Object.keys(edge).sort(), ["dependency", "from", "sha256", "to"]) ||
        !LOCK_ID.test(edge.from ?? "") ||
        !LOCK_ID.test(edge.to ?? "") ||
        typeof edge.dependency !== "string" ||
        !DEPENDENCY_ENTRY.test(edge.dependency) ||
        !isSha(edge.sha256)
      ) {
        throw fail("externalEdges entries need exactly from, dependency, to and sha256");
      }
      const source = changed.find((entry) => entry.id === edge.from);
      if (!source?.addedDependencies.includes(edge.dependency)) {
        throw fail("external edge is not an added dependency of declared changed " + edge.from);
      }
      const key = JSON.stringify([edge.from, edge.dependency]);
      if (seen.has(key)) throw fail("repeated external edge " + key);
      seen.add(key);
    }
    for (const entry of changed) {
      for (const dependency of entry.addedDependencies) {
        if (!seen.has(JSON.stringify([entry.id, dependency]))) {
          throw fail("undeclared external edge " + entry.id + " -> " + dependency);
        }
      }
    }
  }
  return {
    schemaVersion: declaration.schemaVersion,
    producer: declaration.producer,
    oldCargoLockSha256: declaration.oldCargoLockSha256,
    newCargoLockSha256: declaration.newCargoLockSha256,
    added: added.map((entry) => ({
      id: entry.id,
      manifest: entry.manifest,
      sha256: entry.sha256,
      dependencies: sortedUnique(entry.dependencies),
    })),
    changed: changed.map((entry) => ({
      id: entry.id,
      manifest: entry.manifest,
      oldSha256: entry.oldSha256,
      newSha256: entry.newSha256,
      removedDependencies: sortedUnique(entry.removedDependencies),
      addedDependencies: sortedUnique(entry.addedDependencies),
    })),
    closure: { added: sortedUnique(closure.added), changed: sortedUnique(closure.changed) },
    ...(externalMode ? { externalEdges: externalEdges.map((edge) => ({ ...edge })) } : {}),
  };
}

// One `[[package]]` record split into its fields: `dependencies` is the entry
// list (null when the record has none) and `rest` every other line verbatim,
// so two records are the same apart from their edges exactly when their
// `rest` lines are equal.
export function parseLockRecord(body, label) {
  const lines = body.split("\n");
  if (lines.at(-1) !== "") throw rewireError(label + " record does not end with a newline");
  lines.pop();
  const rest = [];
  let dependencies = null;
  let source;
  let checksum;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line === "dependencies = [") {
      if (dependencies !== null) throw rewireError(label + " record repeats dependencies");
      dependencies = [];
      for (i++; i < lines.length && lines[i] !== "]"; i++) {
        const entry = /^ "(.+)",$/.exec(lines[i]);
        if (!entry) throw rewireError(label + " record has an unreadable dependency line");
        dependencies.push(entry[1]);
      }
      if (lines[i] !== "]")
        throw rewireError(label + " record has an unterminated dependencies list");
      continue;
    }
    if (line.startsWith("dependencies = ")) {
      throw rewireError(label + " record has an unexpected dependencies layout");
    }
    if (line.startsWith("source = ")) source = line;
    if (line.startsWith("checksum = ")) checksum = line;
    rest.push(line);
  }
  return { dependencies, rest, source, checksum };
}

// Which record of `records` (the parsed new lock's packages) a Cargo.lock
// dependency entry names, or a problem string: `name` alone must be unique,
// `name version` must match exactly one version, and `name version (source)`
// exactly one source.
export function resolveDependencyEntry(entry, records) {
  const match = DEPENDENCY_ENTRY.exec(entry);
  if (!match) return { problem: "dependency entry " + JSON.stringify(entry) + " is malformed" };
  const [, name, version, source] = match;
  let candidates = records.filter((record) => record.name === name);
  if (version !== undefined) candidates = candidates.filter((r) => r.version === version);
  if (source !== undefined) {
    candidates = candidates.filter((r) => r.fields.source === 'source = "' + source + '"');
  }
  if (candidates.length === 1) return { record: candidates[0] };
  return {
    problem:
      "dependency entry " +
      JSON.stringify(entry) +
      (candidates.length === 0 ? " names no record" : " is ambiguous") +
      " in the new Cargo.lock",
  };
}

// This bounded reader must never interpret string contents as TOML metadata.
function manifestLines(text) {
  return text.split("\n").map((line) => {
    let quote = null;
    for (let i = 0; i < line.length; i++) {
      const char = line[i];
      if (quote !== null) {
        if (quote === '"' && char === "\\") i++;
        else if (char === quote) quote = null;
      } else if (char === "#") {
        return line.slice(0, i);
      } else if (char === '"' || char === "'") {
        if (line.slice(i, i + 3) === char.repeat(3)) {
          throw rewireError("manifest contains unsupported multiline TOML strings; refusing");
        }
        quote = char;
      }
    }
    if (quote !== null)
      throw rewireError("manifest contains an unterminated TOML string; refusing");
    return line;
  });
}

// The `[package]` table of a manifest: name, version and publish, read with a
// minimal single-table TOML reader (quoted strings and booleans only).
export function readManifestPackage(text) {
  const lines = manifestLines(text);
  const start = lines.findIndex((line) => line.trim() === "[package]");
  if (start < 0) return null;
  const table = {};
  for (let i = start + 1; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line.startsWith("[")) break;
    const entry = /^([A-Za-z_-]+)\s*=\s*(.+?)\s*$/.exec(line);
    if (!entry) continue;
    const [, key, raw] = entry;
    if (/^"(?:[^"\\]|\\.)*"$/.test(raw)) table[key] = JSON.parse(raw);
    else if (raw === "true" || raw === "false") table[key] = raw === "true";
    else table[key] = { unsupported: raw };
  }
  return table;
}

// Whether `manifestPath` (a repository-relative Cargo.toml) lies in one of the
// root manifest's `[workspace] members` globs (literal paths and a trailing
// `*` segment, which is how this workspace lists its crates).
export function workspaceMemberProblem(rootManifest, manifestPath) {
  const lines = manifestLines(rootManifest);
  const start = lines.findIndex((line) => line.trim() === "[workspace]");
  if (start < 0) return "the root Cargo.toml has no [workspace] table";
  let end = lines.findIndex((line, i) => i > start && /^\s*\[/.test(line));
  if (end < 0) end = lines.length;
  const table = lines.slice(start + 1, end).join("\n");
  const list = (key) => {
    const match = new RegExp("^" + key + "\\s*=\\s*\\[([^\\]]*)\\]", "m").exec(table);
    return match ? [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]) : null;
  };
  const members = list("members");
  if (!members) return "the root Cargo.toml lists no [workspace] members";
  const dir = dirname(manifestPath).split("/");
  const matches = (glob) => {
    const parts = glob.split("/");
    return parts.length === dir.length && parts.every((part, i) => part === "*" || part === dir[i]);
  };
  if (!members.some(matches)) return manifestPath + " is not a workspace member path";
  if ((list("exclude") ?? []).some(matches)) return manifestPath + " is a workspace-excluded path";
  return null;
}

// Why `manifestPath` does not prove `id` an unpublished workspace crate, or null.
function internalManifestProblem(id, manifestPath, readManifest) {
  let text;
  try {
    text = readManifest(manifestPath);
  } catch (error) {
    return id + ": cannot read " + manifestPath + " (" + error.message + ")";
  }
  const pkg = readManifestPackage(decodeLock(text));
  if (!pkg) return id + ": " + manifestPath + " has no [package] table";
  const [name, version] = id.split("@");
  if (pkg.name !== name || pkg.version !== version) {
    return (
      id +
      ": " +
      manifestPath +
      " is " +
      String(pkg.name) +
      "@" +
      String(typeof pkg.version === "string" ? pkg.version : JSON.stringify(pkg.version)) +
      ", not " +
      id
    );
  }
  if (pkg.publish !== false) {
    return id + ": " + manifestPath + " does not set publish = false";
  }
  let root;
  try {
    root = decodeLock(readManifest("Cargo.toml"));
  } catch (error) {
    return id + ": cannot read the root Cargo.toml (" + error.message + ")";
  }
  const member = workspaceMemberProblem(root, manifestPath);
  return member ? id + ": " + member : null;
}

// Like internalManifestProblem, but returns {problem} or {sha256} of the exact
// manifest bytes the proof read, so the evidence binds them.
function internalManifest(id, manifestPath, readManifest) {
  const problem = internalManifestProblem(id, manifestPath, readManifest);
  if (problem) return { problem };
  return { sha256: sha256(readManifest(manifestPath)) };
}

// The source a `cargo metadata` receipt row must carry for a lock record: the
// record's `source = "..."` value, or null for a path (workspace) record. A git
// lock source ends in `#<commit>`, which the package id's source part omits.
export function lockSourceValue(sourceLine) {
  if (sourceLine === undefined) return null;
  const value = /^source = "(.*)"$/.exec(sourceLine)?.[1] ?? sourceLine;
  return value.startsWith("git+") ? value.replace(/#[0-9a-f]{40}$/, "") : value;
}

// A parsed closure receipt (scripts/record-cargo-producer-closure.mjs) checked
// against the lock it claims; returns its rows keyed by `name@version (source)`.
// Membership and sources are checked against the lock; the enabled features
// are operator-attested by the receipt and cannot be reproduced here.
function readClosureReceipt(receipt, { label, producer, target, lockSha, lockRecords }) {
  const fail = (message) => label + " closure receipt " + message;
  const problems = [];
  if (!isRecord(receipt) || receipt.schemaVersion !== 1) {
    return { problems: [fail("is not a schemaVersion 1 receipt")], rows: new Map() };
  }
  if (receipt.producer !== producer) {
    problems.push(
      fail("was recorded for producer " + String(receipt.producer) + ", not " + producer),
    );
  }
  if (receipt.target !== target) {
    problems.push(fail("was recorded for target " + String(receipt.target) + ", not " + target));
  }
  if (receipt.edges !== "normal,build") problems.push(fail("must record normal,build edges"));
  if (receipt.cargoLockSha256 !== lockSha) {
    problems.push(
      fail("was recorded at Cargo.lock " + String(receipt.cargoLockSha256) + ", not " + lockSha),
    );
  }
  const rows = new Map();
  if (!Array.isArray(receipt.rows) || receipt.rows.length === 0) {
    problems.push(fail("has no rows"));
    return { problems, rows };
  }
  const lockSources = new Map(
    lockRecords.map((record) => [record.id, lockSourceValue(record.fields.source)]),
  );
  for (const row of receipt.rows) {
    if (
      !isRecord(row) ||
      typeof row.name !== "string" ||
      typeof row.version !== "string" ||
      (row.source !== null && typeof row.source !== "string") ||
      !isStringArray(row.features)
    ) {
      problems.push(fail("has a malformed row " + JSON.stringify(row)));
      continue;
    }
    const key = row.name + "@" + row.version + (row.source === null ? "" : " (" + row.source + ")");
    if (rows.has(key)) problems.push(fail("repeats row " + key));
    const id = row.name + "@" + row.version;
    if (!lockSources.has(id)) {
      problems.push(fail("row " + key + " is not a record of its Cargo.lock"));
    } else {
      const expected = lockSources.get(id);
      if (expected === null) {
        if (typeof row.source !== "string" || !row.source.startsWith("path+")) {
          problems.push(
            fail("row " + key + " claims a source, but its Cargo.lock record is a path record"),
          );
        }
      } else if (row.source !== expected) {
        problems.push(
          fail("row " + key + " source differs from its Cargo.lock record's " + expected),
        );
      }
    }
    rows.set(key, { ...row, features: sortedUnique(row.features) });
  }
  if (![...rows.values()].some((row) => row.name === producer)) {
    problems.push(fail("does not contain the producer " + producer));
  }
  return { problems, rows };
}

// Proves one reviewed internal Cargo.lock rewire and returns the `cargoRewire`
// evidence, or throws naming every failed check. Inputs mirror
// proveCargoProducerLock, plus the parsed `declaration` and its verified
// `declarationSource` {path, sha256}, the new lock's reference
// `newLockSource` {sha256[, commit, path]}, the two closure receipts
// (`closures.old`/`closures.new` as {receipt, sha256}), `target`, and
// `readManifest(path)` returning the checkout's bytes for a repository path.
export function proveCargoInternalRewire({
  contractSha,
  artifactSha,
  expectedOldSha,
  expectedNewSha,
  oldText,
  newText,
  oldLockSource,
  newLockSource,
  declaration,
  declarationSource,
  closures,
  target,
  readManifest,
  manifestCommit = null,
}) {
  const problems = [];
  if (manifestCommit !== null && !/^[0-9a-f]{40}$/.test(manifestCommit)) {
    problems.push("manifestCommit must be a full commit SHA or null");
  }
  if (expectedOldSha === expectedNewSha) {
    problems.push("--expected-old-cargo-sha equals --expected-new-cargo-sha; no Cargo.lock move");
  }
  if (contractSha !== expectedOldSha) {
    problems.push("the contract pins Cargo.lock " + contractSha + ", not --expected-old-cargo-sha");
  }
  const oldSha = sha256(oldText ?? "");
  const newSha = sha256(newText ?? "");
  const source = isRecord(oldLockSource)
    ? { commit: oldLockSource.commit, path: oldLockSource.path, sha256: oldLockSource.sha256 }
    : null;
  if (
    !source ||
    !/^[0-9a-f]{40}$/.test(source.commit ?? "") ||
    typeof source.path !== "string" ||
    source.path === "" ||
    source.sha256 !== oldSha
  ) {
    problems.push(
      "the old Cargo.lock source must be its verified git blob {commit, path, sha256}, not " +
        JSON.stringify(oldLockSource),
    );
  }
  let newSource = null;
  try {
    newSource = contractReferenceRecord(newLockSource);
    if (newSource.sha256 !== newSha) {
      problems.push("the new Cargo.lock source digest is not the checkout's Cargo.lock");
    }
  } catch {
    problems.push(
      "the new Cargo.lock source must be {sha256[, commit, path]}, not " +
        JSON.stringify(newLockSource),
    );
  }
  if (oldSha !== expectedOldSha) {
    problems.push(
      (source ? source.commit + ":" + source.path : String(oldLockSource)) +
        " hashes to " +
        oldSha +
        ", not --expected-old-cargo-sha",
    );
  }
  if (newSha !== expectedNewSha) {
    problems.push(
      "the checkout's Cargo.lock hashes to " + newSha + ", not --expected-new-cargo-sha",
    );
  }
  if (artifactSha !== expectedNewSha) {
    problems.push(
      "the artifact measured Cargo.lock " + String(artifactSha) + ", not --expected-new-cargo-sha",
    );
  }
  if (!isRecord(declarationSource) || !isSha(declarationSource.sha256)) {
    problems.push("the declaration source must record its path and sha256");
  }
  if (problems.length > 0) throw rewireError(problems.join("; "));
  const declared = checkRewireDeclaration(declaration);
  const externalMode = declared.schemaVersion === EXISTING_EXTERNAL_EDGE_SCHEMA;
  if (declared.oldCargoLockSha256 !== expectedOldSha) {
    problems.push("the declaration's oldCargoLockSha256 is not --expected-old-cargo-sha");
  }
  if (declared.newCargoLockSha256 !== expectedNewSha) {
    problems.push("the declaration's newCargoLockSha256 is not --expected-new-cargo-sha");
  }
  if (problems.length > 0) throw rewireError(problems.join("; "));

  // Whole-lockfile accounting: header and non-package sections equal, every
  // record paired by identity, every difference declared.
  let before;
  let after;
  try {
    before = parseCargoLock(decodeLock(oldText), "old Cargo.lock");
    after = parseCargoLock(decodeLock(newText), "new Cargo.lock");
  } catch (error) {
    throw rewireError(error.message);
  }
  if (before.header !== after.header) problems.push("the header differs");
  if (!isDeepStrictEqual(before.other, after.other)) problems.push("a non-package section differs");
  const withFields = (lock, label) =>
    lock.packages.map((record) => {
      const [name, version] = record.id.split("@");
      return { ...record, name, version, fields: parseLockRecord(record.body, label) };
    });
  const oldRecords = withFields(before, "old");
  const newRecords = withFields(after, "new");
  for (const [label, records] of [
    ["old", oldRecords],
    ["new", newRecords],
  ]) {
    const seen = new Set();
    for (const { id } of records) {
      if (seen.has(id))
        problems.push("duplicate package identity " + id + " in the " + label + " Cargo.lock");
      seen.add(id);
    }
  }
  const oldById = new Map(oldRecords.map((record) => [record.id, record]));
  const newById = new Map(newRecords.map((record) => [record.id, record]));
  const declaredAdded = new Map(declared.added.map((entry) => [entry.id, entry]));
  const declaredChanged = new Map(declared.changed.map((entry) => [entry.id, entry]));
  const unexplainedChanged = [];
  const removed = [];
  for (const record of oldRecords) {
    const next = newById.get(record.id);
    if (!next) {
      removed.push(record.id);
      continue;
    }
    if (next.body === record.body) continue;
    if (!declaredChanged.has(record.id)) unexplainedChanged.push(record.id);
  }
  const unexplainedAdded = newRecords
    .filter((record) => !oldById.has(record.id) && !declaredAdded.has(record.id))
    .map((record) => record.id);
  if (removed.length > 0) problems.push("removed records " + removed.join(", "));
  if (unexplainedChanged.length > 0) {
    problems.push("undeclared changed records " + unexplainedChanged.join(", "));
  }
  if (unexplainedAdded.length > 0)
    problems.push("undeclared added records " + unexplainedAdded.join(", "));

  const added = [];
  for (const entry of declared.added) {
    const record = newById.get(entry.id);
    if (!record) {
      problems.push("declared added " + entry.id + " is not in the new Cargo.lock");
      continue;
    }
    if (oldById.has(entry.id)) problems.push("declared added " + entry.id + " already existed");
    if (sha256(record.body) !== entry.sha256) {
      problems.push(
        "declared added " + entry.id + " bytes hash to " + sha256(record.body) + ", not its sha256",
      );
    }
    if (record.fields.source !== undefined || record.fields.checksum !== undefined) {
      problems.push(
        "declared added " + entry.id + " has a source or checksum; not an internal record",
      );
    }
    const dependencies = record.fields.dependencies ?? [];
    if (!isDeepStrictEqual(sortedUnique(dependencies), entry.dependencies)) {
      problems.push("declared added " + entry.id + " dependencies differ from the record's");
    }
    for (const dependency of dependencies) {
      const { problem } = resolveDependencyEntry(dependency, newRecords);
      if (problem) problems.push("added " + entry.id + ": " + problem);
    }
    const manifest = internalManifest(entry.id, entry.manifest, readManifest);
    if (manifest.problem) problems.push(manifest.problem);
    added.push({
      id: entry.id,
      manifest: entry.manifest,
      manifestSha256: manifest.sha256 ?? null,
      sha256: entry.sha256,
      dependencies: [...dependencies],
    });
  }
  const internalIds = new Set([...declaredAdded.keys(), ...declaredChanged.keys()]);
  const changed = [];
  for (const entry of declared.changed) {
    const record = oldById.get(entry.id);
    const next = newById.get(entry.id);
    if (!record || !next) {
      problems.push("declared changed " + entry.id + " is not in both Cargo.lock files");
      continue;
    }
    if (record.body === next.body) {
      problems.push("declared changed " + entry.id + " is byte-identical in both Cargo.lock files");
      continue;
    }
    if (sha256(record.body) !== entry.oldSha256) {
      problems.push(
        "declared changed " +
          entry.id +
          " old bytes hash to " +
          sha256(record.body) +
          ", not its oldSha256",
      );
    }
    if (sha256(next.body) !== entry.newSha256) {
      problems.push(
        "declared changed " +
          entry.id +
          " new bytes hash to " +
          sha256(next.body) +
          ", not its newSha256",
      );
    }
    if (!isDeepStrictEqual(record.fields.rest, next.fields.rest)) {
      problems.push("declared changed " + entry.id + " moved a field other than dependencies");
    }
    if (
      [record, next].some((r) => r.fields.source !== undefined || r.fields.checksum !== undefined)
    ) {
      problems.push(
        "declared changed " + entry.id + " has a source or checksum; not an internal record",
      );
    }
    const was = record.fields.dependencies ?? [];
    const now = next.fields.dependencies ?? [];
    if (externalMode && now.length !== new Set(now).size) {
      problems.push("changed " + entry.id + " repeats a dependency entry");
    }
    const removedEdges = sortedUnique(was.filter((d) => !now.includes(d)));
    const addedEdges = sortedUnique(now.filter((d) => !was.includes(d)));
    if (!isDeepStrictEqual(removedEdges, entry.removedDependencies)) {
      problems.push(
        "declared changed " +
          entry.id +
          " removed edges " +
          JSON.stringify(removedEdges) +
          " differ from removedDependencies",
      );
    }
    if (!isDeepStrictEqual(addedEdges, entry.addedDependencies)) {
      problems.push(
        "declared changed " +
          entry.id +
          " added edges " +
          JSON.stringify(addedEdges) +
          " differ from addedDependencies",
      );
    }
    const resolvedAddedTargets = new Set();
    for (const dependency of addedEdges) {
      const { record: targetRecord, problem } = resolveDependencyEntry(dependency, newRecords);
      if (problem) {
        problems.push("changed " + entry.id + ": " + problem);
      } else if (externalMode) {
        const edge = declared.externalEdges.find(
          (e) => e.from === entry.id && e.dependency === dependency,
        );
        const previous = oldById.get(targetRecord.id);
        if (resolvedAddedTargets.has(targetRecord.id)) {
          problems.push(
            "changed " +
              entry.id +
              " repeats an added external target under different dependency spellings",
          );
        }
        resolvedAddedTargets.add(targetRecord.id);
        const oldTarget = resolveDependencyEntry(dependency, oldRecords);
        if (oldTarget.problem || oldTarget.record.id !== targetRecord.id) {
          problems.push(
            "external edge " + dependency + " must resolve to the same record in both locks",
          );
        }
        if (
          was.some(
            (existing) =>
              resolveDependencyEntry(existing, oldRecords).record?.id === targetRecord.id,
          )
        ) {
          problems.push(
            "external edge " +
              entry.id +
              " -> " +
              dependency +
              " already existed under another dependency spelling",
          );
        }
        if (!edge || edge.to !== targetRecord.id || edge.sha256 !== sha256(targetRecord.body)) {
          problems.push(
            "external edge " +
              entry.id +
              " -> " +
              dependency +
              " target identity or bytes differ from declaration",
          );
        }
        if (
          !previous ||
          previous.body !== targetRecord.body ||
          targetRecord.fields.source === undefined
        ) {
          problems.push(
            "external edge target " +
              targetRecord.id +
              " must be an existing byte-identical external record",
          );
        }
      } else if (!internalIds.has(targetRecord.id)) {
        problems.push(
          "changed " +
            entry.id +
            " adds an edge to " +
            targetRecord.id +
            ", which is not a declared internal record",
        );
      }
    }
    const manifest = internalManifest(entry.id, entry.manifest, readManifest);
    if (manifest.problem) problems.push(manifest.problem);
    changed.push({
      id: entry.id,
      manifest: entry.manifest,
      manifestSha256: manifest.sha256 ?? null,
      oldSha256: entry.oldSha256,
      newSha256: entry.newSha256,
      removedDependencies: removedEdges,
      addedDependencies: addedEdges,
    });
  }

  // Producer closures: only declared internal rows may appear or move.
  const closureProblems = [];
  const closureOf = (label, lockSha, lockRecords) => {
    const side = closures?.[label];
    if (!isRecord(side) || !isSha(side.sha256)) {
      closureProblems.push(label + " closure receipt is missing or has no sha256");
      return new Map();
    }
    if (externalMode) {
      const command = side.receipt?.command;
      if (
        !Array.isArray(command) ||
        typeof command[0] !== "string" ||
        !command[0] ||
        /\s-/.test(command[0]) ||
        !isDeepStrictEqual(command.slice(1), [
          "metadata",
          "--locked",
          "--format-version",
          "1",
          "--filter-platform",
          target,
        ])
      ) {
        closureProblems.push(
          label + " closure receipt must attest the default-feature metadata command",
        );
      }
    }
    const { problems: found, rows } = readClosureReceipt(side.receipt, {
      label,
      producer: declared.producer,
      target,
      lockSha,
      lockRecords,
    });
    closureProblems.push(...found);
    return rows;
  };
  const oldRows = closureOf("old", expectedOldSha, oldRecords);
  const newRows = closureOf("new", expectedNewSha, newRecords);
  if (externalMode) {
    for (const [label, rows] of [
      ["old", oldRows],
      ["new", newRows],
    ]) {
      if (
        ![...rows.values()].some(
          (row) => row.name === PRODUCER_PACKAGE && row.features.includes("default"),
        )
      ) {
        closureProblems.push(label + " closure must enable the producer's default feature");
      }
      const ids = new Set([...rows.values()].map((row) => row.name + "@" + row.version));
      for (const edge of declared.externalEdges) {
        if (!ids.has(edge.from) || !ids.has(edge.to)) {
          closureProblems.push(
            label +
              " default-producer closure must already contain both endpoints of " +
              edge.from +
              " -> " +
              edge.to,
          );
        }
      }
    }
  }
  const closure = { added: [], changed: [], removed: [], unchanged: 0 };
  if (closureProblems.length === 0) {
    for (const [key, row] of oldRows) {
      const next = newRows.get(key);
      if (!next) closure.removed.push(key);
      else if (isDeepStrictEqual(row.features, next.features)) closure.unchanged += 1;
      else
        closure.changed.push({
          key,
          id: row.name + "@" + row.version,
          from: row.features,
          to: next.features,
        });
    }
    for (const [key, row] of newRows) {
      if (!oldRows.has(key))
        closure.added.push({ key, id: row.name + "@" + row.version, features: row.features });
    }
    if (closure.removed.length > 0) {
      closureProblems.push("rows left the producer closure: " + closure.removed.join(", "));
    }
    const external = (rows) => rows.filter((row) => !internalIds.has(row.id));
    const externalChanged = external(closure.changed);
    if (externalChanged.length > 0) {
      closureProblems.push(
        "external feature drift in the producer closure: " +
          externalChanged
            .map(
              (row) => row.key + " " + JSON.stringify(row.from) + " -> " + JSON.stringify(row.to),
            )
            .join(", "),
      );
    }
    const externalAdded = external(closure.added);
    if (externalAdded.length > 0) {
      closureProblems.push(
        "external rows joined the producer closure: " +
          externalAdded.map((row) => row.key).join(", "),
      );
    }
    const addedIds = sortedUnique(closure.added.map((row) => row.id));
    const changedIds = sortedUnique(closure.changed.map((row) => row.id));
    if (!isDeepStrictEqual(addedIds, declared.closure.added)) {
      closureProblems.push(
        "closure rows added " +
          JSON.stringify(addedIds) +
          " differ from the declaration's closure.added",
      );
    }
    if (!isDeepStrictEqual(changedIds, declared.closure.changed)) {
      closureProblems.push(
        "closure rows changed " +
          JSON.stringify(changedIds) +
          " differ from the declaration's closure.changed",
      );
    }
  }
  problems.push(...closureProblems);
  if (problems.length > 0) {
    throw rewireError(
      "the Cargo.lock move is not the declared internal rewire (" + problems.join("; ") + ")",
    );
  }
  return {
    oldSha: expectedOldSha,
    newSha: expectedNewSha,
    oldLockSource: source,
    newLockSource: newSource,
    declaration: { path: declarationSource.path, sha256: declarationSource.sha256 },
    producer: declared.producer,
    // Manifests are read from the checkout; `manifestsAtCommit` names the
    // --new-cargo-ref commit whose blobs they were proven byte-identical to,
    // or null when only the working tree was read.
    manifestsAtCommit: manifestCommit,
    added,
    changed,
    removed: [],
    ...(externalMode ? { externalEdges: declared.externalEdges } : {}),
    closure: {
      target,
      oldSha256: closures.old.sha256,
      newSha256: closures.new.sha256,
      rows: { old: oldRows.size, new: newRows.size, unchanged: closure.unchanged },
      added: closure.added.map((row) => ({ id: row.id, features: row.features })),
      changed: closure.changed.map((row) => ({ id: row.id, from: row.from, to: row.to })),
    },
  };
}

// Refuses a cargo-internal-rewire rewrite that moved any key outside
// CARGO_INTERNAL_REWIRE_MOVEMENT.
export function assertCargoInternalRewireMovement(before, after) {
  assertAllowedMovement(
    CARGO_INTERNAL_REWIRE_REASON,
    CARGO_INTERNAL_REWIRE_MOVEMENT,
    before,
    after,
  );
}

function recordList(records, render) {
  return records.length === 0 ? "none" : records.map(render).join(", ");
}

export function cargoRewireText(transition) {
  return (
    "Cargo.lock " +
    transition.oldSha +
    " to " +
    transition.newSha +
    " adds " +
    transition.added.length +
    " internal record(s) (" +
    recordList(transition.added, (r) => r.id) +
    ") and rewires " +
    transition.changed.length +
    " internal record(s) (" +
    recordList(
      transition.changed,
      (r) =>
        r.id +
        " -" +
        JSON.stringify(r.removedDependencies) +
        " +" +
        JSON.stringify(r.addedDependencies),
    ) +
    "); every other [[package]] record and the header are byte-identical, declared in " +
    transition.declaration.path +
    " (SHA-256 " +
    transition.declaration.sha256 +
    "), proved against " +
    transition.oldLockSource.path +
    " at commit " +
    transition.oldLockSource.commit +
    " (SHA-256 " +
    transition.oldLockSource.sha256 +
    "); the " +
    transition.producer +
    " producer closure for " +
    transition.closure.target +
    " keeps " +
    transition.closure.rows.unchanged +
    " rows unchanged, adds " +
    transition.closure.added.length +
    " and changes " +
    transition.closure.changed.length +
    " internal row(s) with no external drift" +
    externalEdgesText(transition, (value) => value)
  );
}

function externalEdgesText(transition, code) {
  if (!transition.externalEdges?.length) return "";
  return (
    "; declared edges to unchanged external records: " +
    recordList(
      transition.externalEdges,
      (edge) =>
        code(edge.from) +
        " " +
        code(edge.dependency) +
        " -> " +
        code(edge.to) +
        " (SHA-256 " +
        code(edge.sha256) +
        ")",
    )
  );
}

export function cargoRewireMarkdown(transition) {
  const code = (value) => "`" + value + "`";
  return (
    "`Cargo.lock` moved from `" +
    transition.oldSha +
    "` to `" +
    transition.newSha +
    "` by adding " +
    transition.added.length +
    " internal record(s) (" +
    recordList(transition.added, (r) => code(r.id)) +
    ") and rewiring " +
    transition.changed.length +
    " internal record(s) (" +
    recordList(
      transition.changed,
      (r) =>
        code(r.id) +
        " removes " +
        recordList(r.removedDependencies, code) +
        ", adds " +
        recordList(r.addedDependencies, code),
    ) +
    "); every other `[[package]]` record and the header are byte-identical, as declared in `" +
    transition.declaration.path +
    "` (SHA-256 `" +
    transition.declaration.sha256 +
    "`) and proved against `" +
    transition.oldLockSource.path +
    "` at commit `" +
    transition.oldLockSource.commit +
    "` (SHA-256 `" +
    transition.oldLockSource.sha256 +
    "`). The `" +
    transition.producer +
    "` producer closure for `" +
    transition.closure.target +
    "` keeps " +
    transition.closure.rows.unchanged +
    " rows unchanged, adds " +
    transition.closure.added.length +
    " and changes " +
    transition.closure.changed.length +
    " internal row(s) with no external package, version, source or feature drift (receipts `" +
    transition.closure.oldSha256 +
    "` and `" +
    transition.closure.newSha256 +
    "`)" +
    externalEdgesText(transition, code) +
    "."
  );
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

export function cargoTransitionMarkdown(transition) {
  if (transition.review) {
    return (
      "`Cargo.lock` moved from `" +
      transition.oldSha +
      "` to `" +
      transition.newSha +
      "` under reviewed non-additive transition `" +
      transition.review.id +
      "` (manifest SHA-256 `" +
      transition.review.manifestSha256 +
      "`): " +
      transition.added.length +
      " new identities, " +
      transition.changed.length +
      " changed records and " +
      transition.removed.length +
      " removed identities, proved against `" +
      transition.oldLockSource.commit +
      ":" +
      transition.oldLockSource.path +
      "`."
    );
  }
  return (
    "`Cargo.lock` moved from `" +
    transition.oldSha +
    "` to `" +
    transition.newSha +
    "` by adding exactly " +
    transition.added.length +
    " records (" +
    transition.added.map((id) => "`" + id + "`").join(", ") +
    "); every previous `[[package]]` record is byte-identical, the header is unchanged, and no " +
    "record changed or was removed, proved against `" +
    transition.oldLockSource.path +
    "` at commit `" +
    transition.oldLockSource.commit +
    "` (SHA-256 `" +
    transition.oldLockSource.sha256 +
    "`)."
  );
}

export function cargoTransitionText(transition) {
  if (transition.review) {
    return (
      "Cargo.lock " +
      transition.oldSha +
      " to " +
      transition.newSha +
      " under reviewed non-additive transition " +
      transition.review.id +
      " (manifest SHA-256 " +
      transition.review.manifestSha256 +
      "): " +
      transition.added.length +
      " new identities, " +
      transition.changed.length +
      " changed records and " +
      transition.removed.length +
      " removed identities, proved against " +
      transition.oldLockSource.commit +
      ":" +
      transition.oldLockSource.path
    );
  }
  return (
    "Cargo.lock " +
    transition.oldSha +
    " to " +
    transition.newSha +
    " adds exactly " +
    transition.added.length +
    " records (" +
    transition.added.join(", ") +
    ") with every previous [[package]] record byte-identical and none changed or removed, proved against " +
    transition.oldLockSource.path +
    " at commit " +
    transition.oldLockSource.commit +
    " (SHA-256 " +
    transition.oldLockSource.sha256 +
    ")"
  );
}

// The immediate predecessor a re-pin records as `baselineEvidence.repinnedFrom`
// (both platforms, same shape and key order): read from the input contract's
// top-level fields only, so earlier evidence is never nested.
export function predecessorRecord(contract) {
  return {
    schemaVersion: contract.schemaVersion,
    outcome: contract.outcome,
    packageVersion: contract.toolchain.packageVersion,
    baselineSourceSha: contract.baselineSourceSha,
    baselineRunId: contract.baselineEvidence?.runId,
  };
}

// `repinnedFrom.contract`: the previous contract's byte digest, plus the commit
// and repository path that store those exact bytes when one was verified
// (previousContractReference). Refuses anything else, so no re-pin is written
// without a reference.
export function contractReferenceRecord(reference) {
  const valid =
    isRecord(reference) &&
    /^[0-9a-f]{64}$/.test(reference.sha256 ?? "") &&
    Object.keys(reference).every((key) => ["sha256", "commit", "path"].includes(key)) &&
    (Object.hasOwn(reference, "commit")
      ? /^[0-9a-f]{40}$/.test(reference.commit ?? "") &&
        typeof reference.path === "string" &&
        reference.path !== ""
      : !Object.hasOwn(reference, "path"));
  if (!valid) {
    throw new Error(
      "a re-pin needs the previous contract reference {sha256[, commit, path]}, found " +
        JSON.stringify(reference),
    );
  }
  return Object.hasOwn(reference, "commit")
    ? { sha256: reference.sha256, commit: reference.commit, path: reference.path }
    : { sha256: reference.sha256 };
}

export function repinnedFromRecord(contract, reference) {
  return { ...predecessorRecord(contract), contract: contractReferenceRecord(reference) };
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
  if (reason === FINGERPRINT_HELPER_REASON) {
    throw new Error("--reason fingerprint-helper takes no artifact; use repinFingerprintHelper");
  }
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
  const repinnedFrom = REPINNED_FROM_REASONS.includes(reason)
    ? repinnedFromRecord(contract, evidence.previousContract)
    : null;
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
  if (reason === "release-bump" || reason === "runtime-identity") {
    assertCargoUnchanged(contract, evidence, reason);
  }
  const dependencyGraph = reason === DEPENDENCY_GRAPH_REASON;
  const fingerprintPolicy = reason === FINGERPRINT_POLICY_REASON;
  if (dependencyGraph) assertDependencyGraphMovement(contract, evidence);
  else if (fingerprintPolicy) assertFingerprintPolicyMovement(contract, evidence);
  else if (contract.schemaVersion === 3) assertFingerprintUnchanged(contract, evidence, reason);
  const fingerprintTransition = dependencyGraph
    ? fingerprintTransitionRecord(contract, evidence.dependencyFingerprint)
    : null;
  const fingerprintPolicyTransition = fingerprintPolicy
    ? fingerprintPolicyTransitionRecord(contract, evidence.dependencyFingerprint)
    : null;
  let cargoTransition = null;
  if (reason === CARGO_PRODUCER_LOCK_REASON) {
    assertCargoOnlyMovement(contract, evidence);
    if (!evidence.cargoLock) throw cargoError("needs the old and new Cargo.lock evidence");
    cargoTransition = proveCargoProducerLock({
      ...evidence.cargoLock,
      contractSha: contract.toolchain.cargoLockSha256,
      artifactSha: evidence.identity.cargoLockSha256,
    });
  }
  const cargoRewireReason = reason === CARGO_INTERNAL_REWIRE_REASON;
  let cargoRewire = null;
  if (cargoRewireReason) {
    assertCargoOnlyMovement(contract, evidence, CARGO_INTERNAL_REWIRE_REASON);
    if (!evidence.cargoRewire) throw rewireError("needs the declared Cargo.lock rewire evidence");
    cargoRewire = proveCargoInternalRewire({
      ...evidence.cargoRewire,
      contractSha: contract.toolchain.cargoLockSha256,
      artifactSha: evidence.identity.cargoLockSha256,
    });
  }
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
  if (cargoTransition) next.toolchain.cargoLockSha256 = cargoTransition.newSha;
  if (cargoRewire) next.toolchain.cargoLockSha256 = cargoRewire.newSha;
  if (fingerprintTransition) next.dependencyFingerprint.sha256 = fingerprintTransition.newSha256;
  if (fingerprintPolicyTransition) {
    next.dependencyFingerprint = { ...fingerprintPolicyTransition.to };
  }
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
  } else if (reason === CARGO_PRODUCER_LOCK_REASON) {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from workflow run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      " after a reviewed " +
      (cargoTransition.review ? "non-additive" : "additive") +
      " Cargo.lock transition (" +
      cargoTransitionText(cargoTransition) +
      "). The producer version, fixtures, runners, pnpm-lock.yaml and its dependency fingerprint, toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; only the Cargo.lock hash moved. Equal totals alone do not prove byte-identical product code. " +
      gzipSummary +
      "." +
      matrices;
  } else if (cargoRewireReason) {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from workflow run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      " after a reviewed internal Cargo.lock rewire (" +
      cargoRewireText(cargoRewire) +
      "). The producer version, fixtures, runners, pnpm-lock.yaml and its dependency fingerprint, toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; only the Cargo.lock hash moved. Equal totals alone do not prove byte-identical product code. " +
      gzipSummary +
      "." +
      matrices;
  } else if (dependencyGraph) {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from workflow run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      " after a reviewed dependency-graph change (" +
      fingerprintTransitionText(fingerprintTransition) +
      "), measured on a checkout already carrying the new graph. The producer version, fixtures, runners, Cargo.lock, toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; only the dependency fingerprint sha256 and the recorded pnpm-lock.yaml hash moved; equal totals alone do not prove byte-identical product code. " +
      gzipSummary +
      "." +
      matrices;
  } else if (fingerprintPolicy) {
    next.platformStatus =
      "Linux x64 has exact zero-allowance v" +
      evidence.version +
      " ceilings on metric " +
      metricText() +
      ", re-pinned from workflow run " +
      evidence.runId +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      " after a reviewed fingerprint-policy change (" +
      fingerprintPolicyTransitionText(fingerprintPolicyTransition) +
      "), measured on a checkout whose lock-fingerprint.mjs exports the new policy. policyVersion and sha256 equal the artifact's run provenance; parser and helperSha256 are proven against the checkout helper, not attested by the artifact. The producer version, fixtures, runners, Cargo.lock, toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; only the dependency fingerprint and the recorded pnpm-lock.yaml hash moved; equal totals alone do not prove byte-identical product code. " +
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
    ...(repinnedFrom ? { repinnedFrom } : {}),
    ...(cargoTransition ? { cargoTransition } : {}),
    ...(cargoRewire ? { cargoRewire } : {}),
    ...(fingerprintTransition ? { fingerprintTransition } : {}),
    ...(fingerprintPolicyTransition ? { fingerprintPolicyTransition } : {}),
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
            : reason === CARGO_PRODUCER_LOCK_REASON
              ? "Reviewed " +
                (cargoTransition.review ? "non-additive" : "additive") +
                " Cargo.lock transition at unchanged v" +
                previousVersion +
                " (" +
                cargoTransitionText(cargoTransition) +
                "); every other reviewed identity input is unchanged; raw and gzipNormalized totals did not change; " +
                gzipSummary +
                "."
              : cargoRewireReason
                ? "Reviewed internal Cargo.lock rewire at unchanged v" +
                  previousVersion +
                  " (" +
                  cargoRewireText(cargoRewire) +
                  "); every other reviewed identity input is unchanged; raw and gzipNormalized totals did not change; " +
                  gzipSummary +
                  "."
                : dependencyGraph
                  ? "Reviewed dependency-graph re-pin at unchanged v" +
                    previousVersion +
                    " (" +
                    fingerprintTransitionText(fingerprintTransition) +
                    "): the new fingerprint was computed by lock-fingerprint.mjs on the checkout and equals the artifact's; every other reviewed identity input, Cargo.lock included, is unchanged; raw and gzipNormalized totals did not change; " +
                    gzipSummary +
                    "."
                  : fingerprintPolicy
                    ? "Reviewed fingerprint-policy re-pin at unchanged v" +
                      previousVersion +
                      " (" +
                      fingerprintPolicyTransitionText(fingerprintPolicyTransition) +
                      "): policyVersion stepped by exactly one to the checkout helper's exported FINGERPRINT_POLICY_VERSION; the new policyVersion and sha256 equal the artifact's, and parser and helperSha256 are the checkout helper's own; every other reviewed identity input, Cargo.lock included, is unchanged; raw and gzipNormalized totals did not change; " +
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
  if (cargoTransition) assertCargoProducerLockMovement(contract, next);
  if (cargoRewire) assertCargoInternalRewireMovement(contract, next);
  if (dependencyGraph) {
    assertAllowedMovement(DEPENDENCY_GRAPH_REASON, DEPENDENCY_GRAPH_MOVEMENT, contract, next);
  }
  if (fingerprintPolicy) {
    assertAllowedMovement(FINGERPRINT_POLICY_REASON, FINGERPRINT_POLICY_MOVEMENT, contract, next);
  }
  return { contract: next, gzipEvidence, previousVersion };
}

export function fingerprintTransitionText(transition) {
  return (
    "dependency fingerprint " +
    transition.oldSha256 +
    " to " +
    transition.newSha256 +
    " under policy v" +
    transition.policyVersion
  );
}

export function fingerprintTransitionMarkdown(transition) {
  return (
    "the dependency fingerprint moved from `" +
    transition.oldSha256 +
    "` to `" +
    transition.newSha256 +
    "` under policy v" +
    transition.policyVersion +
    " ([LOCK-FINGERPRINT-POLICY.md](LOCK-FINGERPRINT-POLICY.md)), measured on a checkout already carrying the new graph, and `lock-fingerprint-inventory.json` was regenerated from `lock-fingerprint.mjs --explain`"
  );
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
  const runKind = [
    "fingerprint-migration",
    CARGO_PRODUCER_LOCK_REASON,
    DEPENDENCY_GRAPH_REASON,
    FINGERPRINT_POLICY_REASON,
    CARGO_INTERNAL_REWIRE_REASON,
  ].includes(reason)
    ? "workflow run "
    : "health run ";
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
    "history." +
    (PAIRED_REASONS.includes(reason)
      ? " The Darwin arm64 contract moves in the same paired change, from its own native " +
        "`island-size-darwin.yml` artifact (`--stage`, then `--apply`).\n"
      : " The Darwin arm64 contract is unchanged; it is re-pinned separately from a native " +
        "`island-size-darwin.yml` artifact by `rebaseline-island-size-darwin.mjs`.\n");
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
  } else if (reason === CARGO_PRODUCER_LOCK_REASON) {
    body =
      "The Linux contract recorded a reviewed " +
      (contract.baselineEvidence.cargoTransition.review ? "non-additive" : "additive") +
      " `Cargo.lock` transition at v" +
      version +
      " from " +
      run +
      ": " +
      cargoTransitionMarkdown(contract.baselineEvidence.cargoTransition) +
      " The producer version, fixtures, runners, `pnpm-lock.yaml` and its dependency fingerprint, " +
      "toolchain and normalizer are unchanged, and every raw and `gzipNormalized` total is " +
      "unchanged; only `toolchain.cargoLockSha256` moved. These totals alone do not prove " +
      "byte-identical product code. " +
      evidence +
      closing;
  } else if (reason === CARGO_INTERNAL_REWIRE_REASON) {
    body =
      "The Linux contract recorded a reviewed internal `Cargo.lock` rewire at v" +
      version +
      " from " +
      run +
      ": " +
      cargoRewireMarkdown(contract.baselineEvidence.cargoRewire) +
      " The producer version, fixtures, runners, `pnpm-lock.yaml` and its dependency fingerprint, " +
      "toolchain and normalizer are unchanged, and every raw and `gzipNormalized` total is " +
      "unchanged; only `toolchain.cargoLockSha256` moved. These totals alone do not prove " +
      "byte-identical product code. " +
      evidence +
      closing;
  } else if (reason === DEPENDENCY_GRAPH_REASON) {
    body =
      "The Linux contract re-pinned `dependencyFingerprint.sha256` at v" +
      version +
      " from " +
      run +
      " after a reviewed dependency-graph change: " +
      fingerprintTransitionMarkdown(contract.baselineEvidence.fingerprintTransition) +
      ". The producer version, fixtures, runners, `Cargo.lock`, toolchain and normalizer are " +
      "unchanged, and every raw and `gzipNormalized` total is unchanged; only " +
      "`dependencyFingerprint.sha256` and the recorded `toolchain.pnpmLockSha256` moved. These " +
      "totals alone do not prove byte-identical product code. " +
      evidence +
      closing;
  } else if (reason === FINGERPRINT_POLICY_REASON) {
    body =
      "The Linux contract re-pinned its dependency fingerprint at v" +
      version +
      " from " +
      run +
      " after a reviewed fingerprint-policy change: " +
      fingerprintPolicyTransitionMarkdown(contract.baselineEvidence.fingerprintPolicyTransition) +
      ". `policyVersion` stepped by exactly one to the checkout helper's exported " +
      "`FINGERPRINT_POLICY_VERSION`. The producer version, fixtures, runners, `Cargo.lock`, " +
      "toolchain and normalizer are unchanged, and every raw and `gzipNormalized` total is " +
      "unchanged; only the four `dependencyFingerprint` keys and the recorded " +
      "`toolchain.pnpmLockSha256` moved. These totals alone do not prove byte-identical product " +
      "code. " +
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

const DARWIN_CONTRACT = "research/v3-island-size/decision.json";
const INVENTORY_FILE = SIZE_DIR + "/lock-fingerprint-inventory.json";
const HELPER_CONTRACTS = { linux: CONTRACT, darwin: DARWIN_CONTRACT };
// The only keys a fingerprint-helper re-pin may write (LOCK-FINGERPRINT-POLICY.md §0).
export const FINGERPRINT_HELPER_MOVEMENT = Object.freeze([
  "dependencyFingerprint.helperSha256",
  "outcome",
  "platformStatus",
]);
const INVENTORY_KEYS = ["importers", "snapshots", "packages", "sections", "counts"];
const NO_NATIVE_RUN =
  "no native run: helperSha256 is outside the hashed input and absent from run provenance (#4198)";

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

// Sorted dotted paths of every leaf that differs between two contracts.
export function contractDiffKeys(before, after, prefix = "") {
  if (!isRecord(before) || !isRecord(after)) {
    return isDeepStrictEqual(before, after) ? [] : [prefix.slice(0, -1)];
  }
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  return [...keys]
    .flatMap((key) =>
      Object.hasOwn(before, key) && Object.hasOwn(after, key)
        ? contractDiffKeys(before[key], after[key], prefix + key + ".")
        : [prefix + key],
    )
    .sort();
}

function short(digest) {
  return String(digest).slice(0, 8);
}

// Runs one lock-fingerprint.mjs source from a temporary sibling of the real
// helper, so its `yaml` import and default repo root resolve exactly as the
// real helper's do; the temporary file is always removed.
export async function computeWithHelperSource(source, root, lockText, currentVersion) {
  const path = join(
    root,
    SIZE_DIR,
    ".lock-fingerprint-repin-" + process.pid + "-" + randomBytes(6).toString("hex") + ".mjs",
  );
  writeFileSync(path, source);
  try {
    const helper = await import(pathToFileURL(path).href);
    return helper.computeLockFingerprint({ lockText, currentVersion, repoRoot: resolve(root) });
  } finally {
    rmSync(path, { force: true });
  }
}

// Repository discovery starts at the supplied root: these variables would make
// git read another repository or worktree instead.
const GIT_REDIRECT_ENV = [
  "GIT_DIR",
  "GIT_WORK_TREE",
  "GIT_INDEX_FILE",
  "GIT_COMMON_DIR",
  "GIT_OBJECT_DIRECTORY",
  "GIT_ALTERNATE_OBJECT_DIRECTORIES",
  "GIT_NAMESPACE",
];

function gitOptions(root) {
  const env = { ...process.env };
  for (const key of GIT_REDIRECT_ENV) delete env[key];
  return { cwd: root, env, maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] };
}

function git(root, args) {
  return execFileSync("git", ["--no-replace-objects", ...args], gitOptions(root));
}

// Like git() but returns {status, stdout, stderr} so a warning on success is visible.
function runGit(root, args) {
  // Local refs/replace must not substitute bytes beneath an immutable citation.
  const result = spawnSync("git", ["--no-replace-objects", ...args], gitOptions(root));
  if (result.error) throw result.error;
  return result;
}

// A ref operand: non-empty, not option-like, and not a `<ref>:<path>` object name.
export function checkGitRef(ref, label) {
  if (typeof ref !== "string" || ref === "" || ref.startsWith("-") || ref.includes(":")) {
    throw new Error(label + " must name a git ref, not " + JSON.stringify(ref));
  }
}

// A canonical repository-relative path: no empty, `.` or `..` segment.
export function checkGitPath(path, label) {
  if (typeof path !== "string" || path === "") throw new Error(label + " has an empty path");
  if (isAbsolute(path) || path.startsWith("/")) {
    throw new Error(label + " path must be relative to the repository root, not " + path);
  }
  const segments = path.split(/[\\/]/);
  if (segments.includes("..")) {
    throw new Error(label + " path must not contain a .. segment: " + path);
  }
  if (segments.some((segment) => segment === "" || segment === ".")) {
    throw new Error(label + " path must be a normalized repository path: " + path);
  }
}

// `<ref>[:<path>]` -> {ref, path}; refnames cannot contain `:`, so the first
// one separates the path, which defaults to `defaultPath`.
export function parseRefSpec(text, defaultPath, label) {
  const spec = String(text ?? "");
  const colon = spec.indexOf(":");
  const ref = colon < 0 ? spec : spec.slice(0, colon);
  const path = colon < 0 ? defaultPath : spec.slice(colon + 1);
  checkGitRef(ref, label);
  checkGitPath(path, label);
  return { ref, path };
}

// Resolves `ref` exactly once, in `root`, to one full commit SHA. `--quiet`
// would silence git's "refname is ambiguous" warning, so it is not passed:
// any stderr output (an ambiguous name or short SHA) refuses, as does a
// non-commit object. There is no fallback ref.
export function resolveCommit(root, ref, label) {
  checkGitRef(ref, label);
  const { status, stdout, stderr } = runGit(root, [
    "-c",
    "core.warnAmbiguousRefs=true",
    "rev-parse",
    "--verify",
    ref + "^{commit}",
  ]);
  const message = stderr.toString().trim();
  const commit = stdout.toString().trim();
  if (status !== 0 || message !== "" || !/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(
      label +
        " " +
        ref +
        " does not resolve to exactly one commit in " +
        root +
        (message ? ": " + message.split("\n").join("; ") : ""),
    );
  }
  return commit;
}

// The raw bytes of `<ref>:<path>` from the checkout at `root`: `ref` resolves
// once to a full commit (resolveCommit), the object must be a blob, and it is
// read only through `git show`. Returns {commit, path, bytes}.
export function readGitBlob(root, ref, path, label = "git blob") {
  checkGitPath(path, label);
  const commit = resolveCommit(root, ref, label);
  const object = commit + ":" + path;
  const type = runGit(root, ["cat-file", "-t", object]);
  if (type.status !== 0) throw new Error(label + " " + object + " does not exist");
  const kind = type.stdout.toString().trim();
  if (kind !== "blob") throw new Error(label + " " + object + " is a " + kind + ", not a blob");
  const shown = runGit(root, ["show", "--no-textconv", object]);
  if (shown.status !== 0) {
    throw new Error(label + " cannot read " + object + ": " + shown.stderr.toString().trim());
  }
  return { commit, path, bytes: shown.stdout };
}

// `file`'s path inside the git checkout at `root`, `/`-separated from its
// top level, or null when `root` is not a checkout or `file` lies outside it.
export function repositoryPath(root, file) {
  const top = runGit(root, ["rev-parse", "--show-toplevel"]);
  if (top.status !== 0) return null;
  let path;
  try {
    path = relative(realpathSync(top.stdout.toString().trim()), realpathSync(file));
  } catch {
    return null;
  }
  if (path === "" || isAbsolute(path) || path.split(sep).includes("..")) return null;
  return path.split(sep).join("/");
}

export const PREVIOUS_CONTRACT_NOTICE =
  "previous contract bytes are not at HEAD; recorded digest only — no retrievable citation";

// The `repinnedFrom.contract` reference for the input contract `bytes` read
// from `file`: always their SHA-256; `commit` and `path` only when the blob at
// that commit is byte-identical. Without `ref`, HEAD is resolved once and a
// mismatch (an edited, uncommitted or reformatted contract, or no checkout)
// records the digest alone with a stderr notice. An explicit `ref` must match
// or the re-pin is refused; it never falls back to HEAD.
export function previousContractReference(
  root,
  file,
  bytes,
  ref,
  notify = (message) => console.error(message),
) {
  const reference = { sha256: sha256(bytes) };
  const label = "--previous-contract-ref";
  const path = repositoryPath(root, file);
  if (ref !== undefined) {
    checkGitRef(ref, label);
    if (!path) {
      throw new Error(label + " " + ref + ": " + file + " is not inside a git checkout at " + root);
    }
    const blob = readGitBlob(root, ref, path, label);
    if (!blob.bytes.equals(Buffer.from(bytes))) {
      throw new Error(
        label +
          " " +
          ref +
          ": " +
          blob.commit +
          ":" +
          path +
          " is not byte-identical to the input contract; refusing",
      );
    }
    return { ...reference, commit: blob.commit, path };
  }
  if (path) {
    try {
      const blob = readGitBlob(root, "HEAD", path, "HEAD");
      if (blob.bytes.equals(Buffer.from(bytes))) {
        return { ...reference, commit: blob.commit, path };
      }
    } catch {
      // No HEAD, or no contract blob at HEAD: digest only, reported below.
    }
  }
  notify("notice: " + PREVIOUS_CONTRACT_NOTICE + " (" + file + ")");
  return reference;
}

// Gathers the evidence for a fingerprint-helper re-pin: both helpers run on the
// checkout's lock and packages/zfb version. A failure of the new helper is the
// checkout's fingerprint error and propagates verbatim.
export async function proveFingerprintHelper(baseRef, root = ".") {
  if (typeof baseRef !== "string" || baseRef.length === 0 || baseRef.startsWith("-")) {
    throw new Error("--base-ref must name a git ref");
  }
  const at = (path) => join(root, path);
  const lockText = readFileSync(at("pnpm-lock.yaml"), "utf8");
  const currentVersion = JSON.parse(readFileSync(at("packages/zfb/package.json"), "utf8")).version;
  const newSource = readFileSync(at(FINGERPRINT_HELPER_FILE));
  let baseSource;
  let baseCommit;
  try {
    baseCommit = git(root, ["rev-parse", "--verify", "--quiet", baseRef + "^{commit}"])
      .toString()
      .trim();
    baseSource = git(root, ["show", baseCommit + ":" + FINGERPRINT_HELPER_FILE]);
  } catch (error) {
    throw new Error(
      "cannot read " +
        FINGERPRINT_HELPER_FILE +
        " at --base-ref " +
        baseRef +
        ": " +
        (error.stderr?.toString().trim() || error.message),
      { cause: error },
    );
  }
  const current = await computeWithHelperSource(newSource, root, lockText, currentVersion);
  let base;
  try {
    base = await computeWithHelperSource(baseSource, root, lockText, currentVersion);
  } catch (error) {
    base = { error: error?.message ?? String(error) };
  }
  return {
    baseRef,
    baseCommit,
    baseHelperSha256: sha256(baseSource),
    newHelperSha256: sha256(newSource),
    current,
    base,
  };
}

function helperRepinError(message) {
  return new Error("--reason fingerprint-helper: " + message);
}

// Re-pins `dependencyFingerprint.helperSha256` of both contracts (#4198), or
// throws naming the reason the change needs instead. Pure: `proof` comes from
// proveFingerprintHelper. Totals are not re-derived; nothing but
// FINGERPRINT_HELPER_MOVEMENT is written, which the exact-key diff enforces.
export function repinFingerprintHelper(contracts, proof) {
  const entries = Object.entries(HELPER_CONTRACTS).map(([name, file]) => {
    if (!isRecord(contracts?.[name])) throw helperRepinError("missing the " + file + " contract");
    return { name, file, contract: contracts[name] };
  });
  for (const { file, contract } of entries) {
    if (contract.schemaVersion !== 3 || !isRecord(contract.dependencyFingerprint)) {
      throw helperRepinError(
        "needs a schemaVersion 3 contract, found " +
          String(contract.schemaVersion) +
          " in " +
          file +
          "; migrate with `fingerprint-migration` first",
      );
    }
  }
  const pinned = entries.map(({ contract }) => contract.dependencyFingerprint.helperSha256);
  if (pinned[0] !== pinned[1]) {
    throw helperRepinError(
      "contracts disagree on helperSha256 (" +
        entries.map(({ file }, i) => file + " " + pinned[i]).join(", ") +
        "); fix the contract that is wrong first",
    );
  }
  const { current, base } = proof;
  for (const { file, contract } of entries) {
    const fingerprint = contract.dependencyFingerprint;
    const policyMoves = ["policyVersion", "parser"].filter(
      (key) => fingerprint[key] !== current[key],
    );
    if (policyMoves.length > 0) {
      throw helperRepinError(
        "policy identity moved (" +
          policyMoves.join(", ") +
          " of " +
          file +
          " differ from the new helper's); this is a `fingerprint-policy` change (#4218), not a helper re-pin",
      );
    }
    if (fingerprint.sha256 !== current.sha256) {
      throw helperRepinError(
        "fingerprint value moved (" +
          file +
          " pins " +
          fingerprint.sha256 +
          ", the new helper computes " +
          current.sha256 +
          "); a reachable dependency change needs `dependency-graph` (#4218), a policy change needs `fingerprint-policy` (#4218)",
      );
    }
  }
  if (proof.newHelperSha256 === pinned[0]) {
    throw helperRepinError("helper unchanged; nothing to re-pin");
  }
  if (proof.baseHelperSha256 !== pinned[0]) {
    throw helperRepinError(
      "the helper at " +
        proof.baseRef +
        " (" +
        proof.baseHelperSha256 +
        ") is not the pinned helper (" +
        pinned[0] +
        "); pass the ref whose helper the contract pins",
    );
  }
  const disagreement = base.error
    ? ["the pinned helper failed: " + base.error]
    : [
        ...["policyVersion", "parser", "sha256"].filter((key) => base[key] !== current[key]),
        ...([base, current].every((side) =>
          isDeepStrictEqual(Object.keys(side.inventory ?? {}).sort(), [...INVENTORY_KEYS].sort()),
        )
          ? INVENTORY_KEYS.filter(
              (key) => !isDeepStrictEqual(base.inventory[key], current.inventory[key]),
            ).map((key) => "inventory." + key)
          : ["inventory"]),
      ];
  if (disagreement.length > 0) {
    throw helperRepinError(
      "old and new helper disagree on the real lock (" +
        disagreement.join(", ") +
        "); this is a `fingerprint-policy` change (#4218)",
    );
  }
  const sentence =
    " Helper re-pinned from " +
    short(pinned[0]) +
    " to " +
    short(proof.newHelperSha256) +
    " under #4198 at " +
    proof.baseRef +
    "; measured inputs and fingerprint unchanged.";
  const rewritten = {};
  for (const { name, contract } of entries) {
    const next = structuredClone(contract);
    next.dependencyFingerprint.helperSha256 = proof.newHelperSha256;
    next.outcome = REASONS[FINGERPRINT_HELPER_REASON].outcome;
    next.platformStatus = contract.platformStatus + sentence;
    assertAllowedMovement(FINGERPRINT_HELPER_REASON, FINGERPRINT_HELPER_MOVEMENT, contract, next);
    rewritten[name] = next;
  }
  return { contracts: rewritten, previousHelperSha256: pinned[0] };
}

function inventoryCounts(inventory) {
  return ["importers", "snapshots", "packages"]
    .map((key) => key + " " + inventory.counts[key].reached + "/" + inventory.counts[key].total)
    .join(", ");
}

export function fingerprintHelperProofLines(proof, previousHelperSha256) {
  return [
    "base-ref: " + proof.baseRef + " (" + proof.baseCommit + ")",
    "old helperSha256: " + previousHelperSha256,
    "new helperSha256: " + proof.newHelperSha256,
    "sha256 (unchanged): " + proof.current.sha256,
    "policyVersion: " + proof.current.policyVersion,
    "parser: " + proof.current.parser,
    "inventory (identical for both helpers): " + inventoryCounts(proof.current.inventory),
    NO_NATIVE_RUN,
  ];
}

export function fingerprintHelperReadmeSection(contract, proof, previousHelperSha256) {
  return (
    "\n## v" +
    contract.toolchain.packageVersion +
    " " +
    REASONS[FINGERPRINT_HELPER_REASON].heading +
    "\n\n" +
    "Both contracts re-pinned `dependencyFingerprint.helperSha256` from `" +
    previousHelperSha256 +
    "` to `" +
    proof.newHelperSha256 +
    "` under [#4198](https://github.com/Takazudo/zudo-front-builder/issues/4198). The pinned helper at `--base-ref " +
    proof.baseRef +
    "` (commit `" +
    proof.baseCommit +
    "`) and the new helper compute the same fingerprint `" +
    proof.current.sha256 +
    "` (policy v" +
    proof.current.policyVersion +
    ", `" +
    proof.current.parser +
    "`) and the same `--explain` inventory (" +
    inventoryCounts(proof.current.inventory) +
    ") on the real lock. No native run: `helperSha256` is outside the hashed input and absent from run provenance. Every other contract key, total and identity input is unchanged.\n"
  );
}

// The checkout's own `lock-fingerprint.mjs --explain` output, as raw bytes.
// The helper's CLI guard compares argv[1] with its resolved module path, and
// its default lock and version are the checkout's.
export function helperExplainBytes(root) {
  const helperPath = realpathSync(join(root, FINGERPRINT_HELPER_FILE));
  return execFileSync(process.execPath, [helperPath, "--explain"], {
    maxBuffer: 64 * 1024 * 1024,
  });
}

// The regenerated lock-fingerprint-inventory.json for a dependency-graph or
// fingerprint-policy re-pin: the checkout's `--explain` bytes, kept verbatim
// once they equal the in-process proof of `helper` (the checkout's own
// lock-fingerprint.mjs) for the same lock, version and build inputs, and once
// that proof is the fingerprint the re-pinned contract pins.
export function regenerateInventory(root, pinned, helper, reason = DEPENDENCY_GRAPH_REASON) {
  const at = (path) => join(root, path);
  const fail = (message) => new Error("--reason " + reason + ": " + message);
  const proof = helper.computeLockFingerprint({
    lockText: readFileSync(at("pnpm-lock.yaml"), "utf8"),
    currentVersion: JSON.parse(readFileSync(at("packages/zfb/package.json"), "utf8")).version,
    repoRoot: resolve(root),
  });
  if (!isDeepStrictEqual(contractFingerprint(proof), pinned)) {
    throw fail("the checkout's fingerprint is not the one the re-pinned contract pins");
  }
  const explain = helperExplainBytes(root);
  const expected = { ...contractFingerprint(proof), inventory: proof.inventory };
  if (!isDeepStrictEqual(JSON.parse(explain.toString("utf8")), expected)) {
    throw fail(
      "the checkout's lock-fingerprint.mjs --explain output disagrees with the fingerprint proof",
    );
  }
  return explain;
}

// Why a saved `measurement.normalized.json` in `modeDir` cannot stand for the
// saved bytes, or null: it must exist, cite the SHA-256 of the saved
// measurement.json and of the current normalizer, and carry results equal to
// `recomputed` (a fresh normalize-identity.mjs pass over the same bytes).
export function savedSidecarProblem(modeDir, recomputed) {
  const sidecarPath = join(modeDir, NORMALIZED_SIDECAR);
  if (!existsSync(sidecarPath)) return "missing " + NORMALIZED_SIDECAR;
  let sidecar;
  try {
    sidecar = JSON.parse(readFileSync(sidecarPath, "utf8"));
  } catch (error) {
    return NORMALIZED_SIDECAR + " is not JSON: " + error.message;
  }
  const measurementSha = sha256(readFileSync(join(modeDir, "measurement.json")));
  if (sidecar?.source?.sha256 !== measurementSha) {
    return (
      NORMALIZED_SIDECAR + " source.sha256 is not the saved measurement.json's (stale sidecar)"
    );
  }
  if (sidecar?.normalizer?.sha256 !== normalizerSha256()) {
    return NORMALIZED_SIDECAR + " normalizer.sha256 differs from the current normalizer";
  }
  if (!isDeepStrictEqual(sidecar.results, JSON.parse(JSON.stringify(recomputed.results)))) {
    return NORMALIZED_SIDECAR + " results disagree with the recomputation from the saved bytes";
  }
  return null;
}

export const STAGE_MANIFEST = "stage-manifest.json";
export const POLICY_DOC = SIZE_DIR + "/" + POLICY_DOC_NAME;
// Shared outputs both platforms rewrite: a later stage reads the staged copy.
export const STAGE_OVERLAY_FILES = Object.freeze([TEST_FILE, README, INVENTORY_FILE, POLICY_DOC]);
// Every live file either script can stage, checked before the stage directory is read.
const STAGEABLE_FILES = [CONTRACT, DARWIN_CONTRACT, ...STAGE_OVERLAY_FILES];
// Derived whole from the checkout and the re-pinned identity, so every stage
// must produce the same bytes.
const STAGE_IDENTICAL_FILES = [INVENTORY_FILE, POLICY_DOC];

// `path` resolved through every existing symlink, even when its tail does not exist yet.
function realTarget(path) {
  let existing = resolve(path);
  const rest = [];
  while (!existsSync(existing)) {
    const parent = dirname(existing);
    if (parent === existing) break;
    rest.unshift(basename(existing));
    existing = parent;
  }
  return join(realpathSync(existing), ...rest);
}

function isWithin(child, parent) {
  return child === parent || child.startsWith(parent + sep);
}

function hashOrNull(path) {
  return existsSync(path) ? sha256(readFileSync(path)) : null;
}

function listFiles(dir, prefix = "") {
  return readdirSync(join(dir, prefix), { withFileTypes: true }).flatMap((entry) => {
    const path = prefix === "" ? entry.name : prefix + "/" + entry.name;
    return entry.isDirectory() ? listFiles(dir, path) : [path];
  });
}

// A stage directory must not be, contain or alias the checkout or any of the
// live `paths`, so staging can never write a live destination.
function checkStageLocation(root, dir, paths, label) {
  const stage = realTarget(dir);
  const top = realpathSync(root);
  if (isWithin(top, stage)) {
    throw new Error(label + ": the stage directory resolves to or contains the checkout " + top);
  }
  for (const path of paths) {
    const live = realTarget(resolve(root, path));
    if (isWithin(live, stage)) {
      throw new Error(label + ": the stage directory contains the live destination " + path);
    }
    if (realTarget(join(dir, path)) === live) {
      throw new Error(label + ": the staged " + path + " aliases the live destination");
    }
  }
}

function isSha(value) {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

// Reads and verifies `<dir>/stage-manifest.json`: every listed staged file
// still hashes to its recorded value and no unlisted file is present. Returns
// null for a missing or empty directory when the manifest is not required.
function readStageManifest(dir, label, required) {
  const files = existsSync(dir) ? listFiles(dir) : [];
  const path = join(dir, STAGE_MANIFEST);
  if (!existsSync(path)) {
    if (required) throw new Error(label + ": no " + STAGE_MANIFEST + " in the stage directory");
    if (files.length > 0) {
      throw new Error(
        label +
          ": the stage directory is not empty and has no " +
          STAGE_MANIFEST +
          "; refusing (" +
          STAGE_RECOVERY +
          ")",
      );
    }
    return null;
  }
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Error(label + ": " + STAGE_MANIFEST + " is not JSON: " + error.message, {
      cause: error,
    });
  }
  const destinations = manifest?.destinations;
  if (
    !isRecord(manifest) ||
    manifest.stageVersion !== 1 ||
    !Array.isArray(manifest.runs) ||
    !manifest.runs.every((run) => isRecord(run) && typeof run.reason === "string") ||
    !isRecord(destinations) ||
    !Object.values(destinations).every(
      (entry) =>
        isRecord(entry) && isSha(entry.staged) && (entry.live === null || isSha(entry.live)),
    )
  ) {
    throw new Error(label + ": " + STAGE_MANIFEST + " is malformed");
  }
  for (const [destination, entry] of Object.entries(destinations)) {
    checkGitPath(destination, label + " destination");
    if (hashOrNull(join(dir, destination)) !== entry.staged) {
      throw new Error(
        label +
          ": the staged " +
          destination +
          " changed after staging; refusing (" +
          STAGE_RECOVERY +
          ")",
      );
    }
  }
  const unlisted = files.filter(
    (file) => file !== STAGE_MANIFEST && !Object.hasOwn(destinations, file),
  );
  if (unlisted.length > 0) {
    throw new Error(
      label +
        ": unlisted files in the stage directory: " +
        unlisted.join(", ") +
        " (" +
        STAGE_RECOVERY +
        ")",
    );
  }
  return manifest;
}

// Where a run's rendered outputs go. Without `stageDir`, `commit` writes the
// live files under `root`. With it, `read` returns the staged copy of a shared
// output an earlier stage wrote, and `commit` writes every output under
// `stageDir` (mirroring its repository path) plus a manifest recording, per
// destination, the live file's SHA-256 at staging time and the staged
// output's. `commit` validates everything before its first write. `io` swaps
// the staging writes for a test that forces a failure mid-commit: the files
// are written one by one and the manifest last, so a failed commit can leave a
// partial stage, which readStageManifest then refuses (unlisted or changed
// files, or files without a manifest).
export function openOutputSession(root, stageDir, io = { writeFileSync, mkdirSync }) {
  if (stageDir === undefined) {
    return {
      read: (path) => readFileSync(resolve(root, path)),
      commit(outputs) {
        for (const { path, bytes } of outputs) writeFileSync(resolve(root, path), bytes);
      },
    };
  }
  const label = "--stage " + stageDir;
  checkStageLocation(root, stageDir, STAGEABLE_FILES, label);
  const manifest = readStageManifest(stageDir, label, false);
  const staged = (path) =>
    manifest !== null &&
    Object.hasOwn(manifest.destinations, path) &&
    STAGE_OVERLAY_FILES.includes(path);
  return {
    read: (path) => readFileSync(staged(path) ? join(stageDir, path) : resolve(root, path)),
    commit(outputs, run) {
      const paths = outputs.map(({ path }) => path);
      for (const path of paths) checkGitPath(path, label + " destination");
      checkStageLocation(root, stageDir, paths, label);
      const destinations = { ...manifest?.destinations };
      for (const [path, entry] of Object.entries(destinations)) {
        if (hashOrNull(resolve(root, path)) !== entry.live) {
          throw new Error(
            label + ": live " + path + " moved since it was staged; start a new stage directory",
          );
        }
      }
      for (const { path, bytes } of outputs) {
        const previous = destinations[path];
        if (previous && !STAGE_OVERLAY_FILES.includes(path)) {
          throw new Error(
            label + ": " + path + " is already staged; stage each platform once per directory",
          );
        }
        if (previous && STAGE_IDENTICAL_FILES.includes(path) && sha256(bytes) !== previous.staged) {
          throw new Error(label + ": the staged " + path + " disagrees with this checkout's");
        }
        destinations[path] = {
          live: previous ? previous.live : hashOrNull(resolve(root, path)),
          staged: sha256(bytes),
        };
      }
      const runs = [...(manifest?.runs ?? []), run];
      const problem = stageRunsProblem(runs);
      if (problem) throw new Error(label + ": " + problem + " (nothing was staged)");
      const next = {
        stageVersion: 1,
        runs,
        destinations: Object.fromEntries(
          Object.keys(destinations)
            .sort()
            .map((path) => [path, destinations[path]]),
        ),
      };
      try {
        for (const { path, bytes } of outputs) {
          io.mkdirSync(dirname(join(stageDir, path)), { recursive: true });
          io.writeFileSync(join(stageDir, path), bytes);
        }
        io.writeFileSync(join(stageDir, STAGE_MANIFEST), JSON.stringify(next, null, 2) + "\n");
      } catch (error) {
        throw new Error(
          label +
            ": staging failed (" +
            error.message +
            "); the stage directory may now hold a partial stage, which --apply and --stage refuse: " +
            STAGE_RECOVERY,
          { cause: error },
        );
      }
    },
  };
}

// The platform names a stage run records; a paired reason needs both.
export const STAGE_PLATFORMS = Object.freeze(["linux-x64", "darwin-arm64"]);

const STAGE_RECOVERY = "discard the stage directory and stage again from the start";

// The prior and re-pinned identity a paired run stages. Both platforms must
// record equal values: one package version, Cargo.lock and dependency
// fingerprint before, and the same plus one pnpm-lock.yaml after. The prior
// pnpm-lock.yaml hash is left out because `lockfile-provenance` moves it on
// Linux alone.
export function pairedTransition(previous, next) {
  const identity = (contract) => ({
    packageVersion: contract.toolchain.packageVersion,
    cargoLockSha256: contract.toolchain.cargoLockSha256,
    dependencyFingerprint: contract.dependencyFingerprint,
  });
  return {
    from: identity(previous),
    to: { ...identity(next), pnpmLockSha256: next.toolchain.pnpmLockSha256 },
  };
}

// What a paired reason's stage run records beyond its measured source: the
// previous contract reference (per platform, never compared: the two contracts
// are different files) and the transition both runs must share; {} otherwise.
export function stageRunIdentity(reason, previous, next) {
  if (!PAIRED_REASONS.includes(reason)) return {};
  const rewire = next.baselineEvidence.cargoRewire;
  return {
    previousContract: next.baselineEvidence.repinnedFrom.contract,
    transition: pairedTransition(previous, next),
    // A rewire additionally binds both platforms to one reviewed declaration
    // and one old/new lock pair (pairedRunsProblem compares it).
    ...(reason === CARGO_INTERNAL_REWIRE_REASON
      ? {
          rewire: {
            declarationSha256: rewire.declaration.sha256,
            oldLockSha256: rewire.oldSha,
            newLockSha256: rewire.newSha,
          },
        }
      : {}),
  };
}

function identityText(value) {
  return typeof value === "string" ? value : JSON.stringify(value);
}

// Why a paired reason's Linux and Darwin runs are not one re-pin, or null.
// Their transitions must be equal. Their measured sources may differ in one
// way only: Linux measures health.yml's PR merge source, whose probe head has
// the identical tree (sourceText), so Darwin may have measured either that
// merge source or the Linux probe head; any other source is refused.
function pairedRunsProblem(reason, linux, darwin) {
  const problems = [];
  for (const run of [linux, darwin]) {
    if (!isRecord(run.transition)) {
      problems.push(run.platform + " run records no transition identity");
    }
  }
  if (problems.length === 0) {
    for (const side of ["from", "to"]) {
      const l = linux.transition[side] ?? {};
      const d = darwin.transition[side] ?? {};
      for (const key of new Set([...Object.keys(l), ...Object.keys(d)])) {
        if (isDeepStrictEqual(l[key], d[key])) continue;
        problems.push(
          "transition " +
            side +
            "." +
            key +
            " differs (linux-x64 " +
            identityText(l[key]) +
            ", darwin-arm64 " +
            identityText(d[key]) +
            ")",
        );
      }
    }
  }
  if (reason === CARGO_INTERNAL_REWIRE_REASON) {
    for (const run of [linux, darwin]) {
      if (!isRecord(run.rewire)) problems.push(run.platform + " run records no rewire identity");
    }
    if (isRecord(linux.rewire) && isRecord(darwin.rewire)) {
      for (const key of ["declarationSha256", "oldLockSha256", "newLockSha256"]) {
        if (linux.rewire[key] !== darwin.rewire[key]) {
          problems.push(
            "rewire " +
              key +
              " differs (linux-x64 " +
              identityText(linux.rewire[key]) +
              ", darwin-arm64 " +
              identityText(darwin.rewire[key]) +
              ")",
          );
        }
      }
    }
  }
  if (darwin.sourceSha !== linux.sourceSha && darwin.sourceSha !== linux.headSha) {
    problems.push(
      "darwin-arm64 measured source " +
        darwin.sourceSha +
        " is neither the linux-x64 measured source " +
        linux.sourceSha +
        " nor its probe head " +
        linux.headSha,
    );
  }
  return problems.length === 0
    ? null
    : "--reason " +
        reason +
        " paired runs disagree: " +
        problems.join("; ") +
        "; discard the stage directory and stage both platforms from one checkout and one measured candidate";
}

// Why a stage manifest's `runs` cannot be one stage, or null: a directory
// holds one reason, a paired reason stages each platform at most once, and
// once both are staged they must agree (pairedRunsProblem).
function stageRunsProblem(runs) {
  const reasons = [...new Set(runs.map((run) => run.reason))].sort();
  if (reasons.length > 1) {
    return (
      "the stage directory mixes --reason " +
      reasons.join(", ") +
      "; stage one reason per directory"
    );
  }
  const [reason] = reasons;
  if (!PAIRED_REASONS.includes(reason)) return null;
  const unknown = runs.filter((run) => !STAGE_PLATFORMS.includes(run.platform));
  if (unknown.length > 0) {
    return "--reason " + reason + " staged an unknown platform " + String(unknown[0].platform);
  }
  const [linux, darwin] = STAGE_PLATFORMS.map((platform) =>
    runs.filter((run) => run.platform === platform),
  );
  if (linux.length > 1 || darwin.length > 1) {
    return "--reason " + reason + " staged a platform twice";
  }
  return linux.length === 1 && darwin.length === 1
    ? pairedRunsProblem(reason, linux[0], darwin[0])
    : null;
}

// Lands `targets` ({path, target, bytes, original}) in two phases: every
// destination's bytes go to a sibling temporary file first, then each is
// renamed over its destination. A failure before the first rename removes the
// temporary files and changes nothing; a failed rename restores the
// destinations already renamed from their `original` bytes (null: the file did
// not exist), best effort, and reports what it could not restore. `io` swaps
// the write and rename for a test that forces a failure.
function landAll(targets, label, io) {
  const temps = [];
  try {
    for (const { target, bytes } of targets) {
      temps.push(target + ".apply-" + randomBytes(8).toString("hex") + ".tmp");
      io.writeFileSync(temps.at(-1), bytes);
    }
  } catch (error) {
    for (const temp of temps) rmSync(temp, { force: true });
    throw new Error(
      label + ": could not write a temporary file (" + error.message + "); nothing was applied",
      { cause: error },
    );
  }
  let renamed = 0;
  try {
    for (; renamed < targets.length; renamed++)
      io.renameSync(temps[renamed], targets[renamed].target);
  } catch (error) {
    const unrestored = [];
    for (const { path, target, original } of targets.slice(0, renamed)) {
      try {
        if (original === null) rmSync(target, { force: true });
        else writeFileSync(target, original);
      } catch {
        unrestored.push(path);
      }
    }
    for (const temp of temps.slice(renamed)) rmSync(temp, { force: true });
    throw new Error(
      label +
        ": renaming " +
        targets[renamed].path +
        " into place failed (" +
        error.message +
        "); " +
        (unrestored.length === 0
          ? "restored the " + renamed + " destination(s) already renamed, so nothing was applied"
          : "could not restore " + unrestored.join(", ") + "; restore them from git"),
      { cause: error },
    );
  }
}

// `--apply <dir>`: re-validates that every live destination still hashes to
// its manifest "live" value and every staged file to its "staged" value, and
// that the runs are one stage (stageRunsProblem: one reason, and a paired
// reason staged for both platforms with one transition and one measured
// candidate); then lands every destination through landAll. Any mismatch
// writes nothing.
export function applyStage(
  stageDir,
  root = ".",
  log = console.log,
  io = { writeFileSync, renameSync },
) {
  const label = "--apply " + stageDir;
  checkStageLocation(root, stageDir, STAGEABLE_FILES, label);
  const manifest = readStageManifest(stageDir, label, true);
  const paths = Object.keys(manifest.destinations);
  if (paths.length === 0) throw new Error(label + ": nothing is staged");
  checkStageLocation(root, stageDir, paths, label);
  const problem = stageRunsProblem(manifest.runs);
  if (problem) throw new Error(label + ": " + problem + " (nothing was applied)");
  for (const reason of new Set(manifest.runs.map((run) => run.reason))) {
    if (!PAIRED_REASONS.includes(reason)) continue;
    const platforms = manifest.runs
      .filter((run) => run.reason === reason)
      .map((run) => run.platform);
    const missing = STAGE_PLATFORMS.filter((platform) => !platforms.includes(platform));
    if (missing.length > 0) {
      throw new Error(
        label +
          ": --reason " +
          reason +
          " is not staged for " +
          missing.join(", ") +
          "; stage both platforms into the same directory first (nothing was applied)",
      );
    }
  }
  // Each destination's real file, so a symlinked destination is written through.
  const targets = paths.map((path) => {
    const target = realTarget(resolve(root, path));
    return { path, target, original: existsSync(target) ? readFileSync(target) : null };
  });
  const moved = targets
    .filter(
      ({ path, original }) =>
        (original === null ? null : sha256(original)) !== manifest.destinations[path].live,
    )
    .map(({ path }) => path);
  if (moved.length > 0) {
    throw new Error(label + ": " + moved.join(", ") + " moved since staging; nothing was applied");
  }
  for (const entry of targets) {
    entry.bytes = readFileSync(join(stageDir, entry.path));
    if (sha256(entry.bytes) !== manifest.destinations[entry.path].staged) {
      throw new Error(
        label + ": the staged " + entry.path + " changed after staging; nothing was applied",
      );
    }
  }
  landAll(targets, label, io);
  log(
    "applied " +
      paths.length +
      " staged files (" +
      manifest.runs.map((run) => run.platform + " " + run.reason).join(", ") +
      ")",
  );
}

// A paired reason moves files both platforms share, so one platform never
// writes them live: each stages into one directory, and `--apply` lands both.
export function checkPairedReasonStage(values) {
  if (!PAIRED_REASONS.includes(values.reason) || Object.hasOwn(values, "stage")) return;
  throw new Error(
    "--reason " +
      values.reason +
      " re-pins both platforms together and only stages: pass --stage <dir> for Linux x64 and Darwin arm64 into the same directory, then --apply <dir> (nothing was written)",
  );
}

// `--apply <dir>` takes no other argument; returns whether it was given.
export function checkApplyArgs(values) {
  if (!Object.hasOwn(values, "apply")) return false;
  for (const key of Object.keys(values)) {
    if (key !== "apply") throw new Error("--apply takes no other argument; remove --" + key);
  }
  if (!values.apply) throw new Error("--apply needs a stage directory");
  return true;
}

// The artifact-less CLI path shared by both platform helpers: one run rewrites
// both contracts, the inventory and the README together.
export async function runFingerprintHelperRepin(baseRef, root = ".", log = console.log) {
  const at = (path) => join(root, path);
  const contracts = Object.fromEntries(
    Object.entries(HELPER_CONTRACTS).map(([name, file]) => [
      name,
      JSON.parse(readFileSync(at(file), "utf8")),
    ]),
  );
  const proof = await proveFingerprintHelper(baseRef, root);
  const result = repinFingerprintHelper(contracts, proof);
  const explain = helperExplainBytes(root);
  const { current } = proof;
  const expected = {
    policyVersion: current.policyVersion,
    parser: current.parser,
    helperSha256: proof.newHelperSha256,
    sha256: current.sha256,
    inventory: current.inventory,
  };
  if (!isDeepStrictEqual(JSON.parse(explain.toString()), expected)) {
    throw helperRepinError("the new helper's --explain output disagrees with the proof");
  }
  const readme = fingerprintHelperReadmeSection(
    result.contracts.linux,
    proof,
    result.previousHelperSha256,
  );
  for (const [name, file] of Object.entries(HELPER_CONTRACTS)) {
    writeFileSync(at(file), JSON.stringify(result.contracts[name], null, 2) + "\n");
  }
  writeFileSync(at(INVENTORY_FILE), explain);
  appendFileSync(at(README), readme);
  log(
    [
      "fingerprint-helper re-pin of both island-size contracts (#4198):",
      ...fingerprintHelperProofLines(proof, result.previousHelperSha256).map((line) => "  " + line),
    ].join("\n"),
  );
  return result;
}

// Arguments that name artifact evidence; the artifact-less reason refuses them all.
export const ARTIFACT_ARGS = Object.freeze([
  "artifact",
  "run-id",
  "job-id",
  "head-sha",
  "source-sha",
  "artifact-id",
  "artifact-expires-at",
  "label",
]);

// `values` are the parsed `--key value` pairs. fingerprint-helper takes exactly
// --reason and --base-ref; --base-ref belongs to no other reason.
export function checkFingerprintHelperArgs(values) {
  if (values.reason !== FINGERPRINT_HELPER_REASON) {
    if (Object.hasOwn(values, "base-ref")) {
      throw new Error("--base-ref is only for --reason fingerprint-helper");
    }
    return false;
  }
  for (const key of Object.keys(values)) {
    if (key === "reason" || key === "base-ref") continue;
    throw new Error(
      ARTIFACT_ARGS.includes(key)
        ? "--reason fingerprint-helper takes no artifact (no native run); remove --" + key
        : "--reason fingerprint-helper takes only --base-ref; remove --" + key,
    );
  }
  if (!values["base-ref"]) throw new Error("required --base-ref");
  return true;
}

export function parseArgs(argv) {
  const values = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined)
      throw new Error("bad argument: " + argv[i]);
    values[argv[i].slice(2)] = argv[i + 1];
  }
  if (checkApplyArgs(values)) return values;
  if (checkFingerprintHelperArgs(values)) return values;
  for (const required of ["artifact", "run-id", "job-id", "head-sha", "reason"]) {
    if (!values[required]) throw new Error("required --" + required);
  }
  for (const id of ["run-id", "job-id", "artifact-id"]) {
    if ((id !== "artifact-id" || Object.hasOwn(values, id)) && !/^[1-9][0-9]*$/.test(values[id]))
      throw new Error("--" + id + " must be a positive integer");
  }
  if (!/^[0-9a-f]{40}$/.test(values["head-sha"])) {
    throw new Error("--head-sha must be a full 40-character commit SHA");
  }
  if (!Object.hasOwn(REASONS, values.reason)) {
    throw new Error("--reason must be one of " + Object.keys(REASONS).join(", "));
  }
  checkSourceShaArg(values);
  if (Object.hasOwn(values, "stage") && !values.stage) {
    throw new Error("--stage needs a directory");
  }
  checkPairedReasonStage(values);
  checkCargoInternalRewireArgs(values);
  checkCargoProducerLockArgs(values);
  checkPreviousContractRefArgs(values);
  return values;
}

// `--source-sha` names the measured source the artifact must carry (the PR
// merge source when health.yml measured a PR; `--head-sha` stays the probe
// head). Required by SOURCE_BOUND_REASONS, validated wherever it is given.
export function checkSourceShaArg(values) {
  if (!Object.hasOwn(values, "source-sha")) {
    if (SOURCE_BOUND_REASONS.includes(values.reason)) throw new Error("required --source-sha");
    return;
  }
  if (!/^[0-9a-f]{40}$/.test(values["source-sha"])) {
    throw new Error("--source-sha must be a full 40-character commit SHA");
  }
}

// The cargo-producer-lock evidence a CLI run passes to the pure proof: the
// operator's expected hashes and allowlist, the old lock's git blob at
// `--old-cargo-ref` in `root` and the checkout's own Cargo.lock, both as raw
// bytes. `--old-cargo-lock`, when given, must be byte-identical to the blob.
export function readCargoLockEvidence(args, root) {
  const { ref, path } = parseOldCargoRef(args["old-cargo-ref"]);
  let blob;
  try {
    blob = readGitBlob(root, ref, path, "--old-cargo-ref");
  } catch (error) {
    throw cargoError(error.message);
  }
  if (Object.hasOwn(args, "old-cargo-lock")) {
    if (!readFileSync(args["old-cargo-lock"]).equals(blob.bytes)) {
      throw cargoError("--old-cargo-lock differs from " + blob.commit + ":" + blob.path);
    }
  }
  return {
    expectedOldSha: args["expected-old-cargo-sha"],
    expectedNewSha: args["expected-new-cargo-sha"],
    added: parseAddedPackages(args["added-packages"]),
    oldText: blob.bytes,
    newText: readFileSync(join(root, "Cargo.lock")),
    oldLockSource: { commit: blob.commit, path: blob.path, sha256: sha256(blob.bytes) },
    ...(args["reviewed-cargo-transition"]
      ? {
          reviewedTransitionText: readFileSync(args["reviewed-cargo-transition"], "utf8"),
        }
      : {}),
  };
}

export const LINUX_PRODUCER_TARGET = "x86_64-unknown-linux-gnu";

// The reference recorded for the new Cargo.lock bytes `bytes` of the checkout
// at `root`: always their SHA-256; `commit` and `path` when `--new-cargo-ref`
// (which must be byte-identical, never falling back) or, without it, HEAD
// stores exactly those bytes.
export function newLockReference(root, bytes, ref, notify = (message) => console.error(message)) {
  const reference = { sha256: sha256(bytes) };
  if (ref !== undefined) {
    const spec = parseNewCargoRef(ref);
    let blob;
    try {
      blob = readGitBlob(root, spec.ref, spec.path, "--new-cargo-ref");
    } catch (error) {
      throw rewireError(error.message);
    }
    if (!blob.bytes.equals(Buffer.from(bytes))) {
      throw rewireError(
        "--new-cargo-ref " +
          ref +
          ": " +
          blob.commit +
          ":" +
          blob.path +
          " is not byte-identical to the checkout's Cargo.lock; refusing",
      );
    }
    return { ...reference, commit: blob.commit, path: blob.path };
  }
  try {
    const blob = readGitBlob(root, "HEAD", OLD_CARGO_LOCK_PATH, "HEAD");
    if (blob.bytes.equals(Buffer.from(bytes))) {
      return { ...reference, commit: blob.commit, path: blob.path };
    }
  } catch {
    // No HEAD or no Cargo.lock at HEAD: digest only, reported below.
  }
  notify(
    "notice: new Cargo.lock bytes are not at HEAD; recorded digest only (pass --new-cargo-ref to cite a commit)",
  );
  return reference;
}

function readJsonFile(path, label) {
  let bytes;
  try {
    bytes = readFileSync(path);
  } catch (error) {
    throw rewireError(label + " " + path + " cannot be read: " + error.message);
  }
  let value;
  try {
    value = JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    throw rewireError(label + " " + path + " is not JSON: " + error.message);
  }
  return { bytes, value, sha256: sha256(bytes) };
}

// The cargo-internal-rewire evidence a CLI run passes to the pure proof: the
// shared old-lock blob and checkout lock (readCargoLockEvidence's inputs), the
// new lock's reference, the parsed declaration with its digest, both closure
// receipts with their digests, this platform's producer `target`, and a
// manifest reader rooted at the checkout.
export function readCargoRewireEvidence(args, root, target = LINUX_PRODUCER_TARGET) {
  const { ref, path } = parseOldCargoRef(args["old-cargo-ref"]);
  let blob;
  try {
    blob = readGitBlob(root, ref, path, "--old-cargo-ref");
  } catch (error) {
    throw rewireError(error.message);
  }
  if (Object.hasOwn(args, "old-cargo-lock")) {
    if (!readFileSync(args["old-cargo-lock"]).equals(blob.bytes)) {
      throw rewireError("--old-cargo-lock differs from " + blob.commit + ":" + blob.path);
    }
  }
  const newText = readFileSync(join(root, OLD_CARGO_LOCK_PATH));
  const declaration = readJsonFile(args["rewire-declaration"], "--rewire-declaration");
  const declarationPath = repositoryPath(root, args["rewire-declaration"]);
  const closures = {
    old: readJsonFile(args["old-closure"], "--old-closure"),
    new: readJsonFile(args["new-closure"], "--new-closure"),
  };
  const newLockSource = newLockReference(root, newText, args["new-cargo-ref"]);
  return {
    expectedOldSha: args["expected-old-cargo-sha"],
    expectedNewSha: args["expected-new-cargo-sha"],
    oldText: blob.bytes,
    newText,
    oldLockSource: { commit: blob.commit, path: blob.path, sha256: sha256(blob.bytes) },
    newLockSource,
    declaration: declaration.value,
    // The declaration is cited by its repository path when it lives in the
    // checkout, otherwise by the path given; its digest is always recorded.
    declarationSource: {
      path: declarationPath ?? args["rewire-declaration"],
      sha256: declaration.sha256,
    },
    closures: {
      old: { receipt: closures.old.value, sha256: closures.old.sha256 },
      new: { receipt: closures.new.value, sha256: closures.new.sha256 },
    },
    target,
    manifestCommit: newLockSource.commit ?? null,
    // Every cited commit, including an automatically discovered HEAD, must
    // contain the exact manifest bytes read by the proof.
    readManifest: (manifest) => {
      checkGitPath(manifest, "manifest");
      const bytes = readFileSync(join(root, manifest));
      if (newLockSource.commit) {
        let blob;
        try {
          blob = readGitBlob(root, newLockSource.commit, manifest, "--new-cargo-ref manifest");
        } catch (error) {
          throw rewireError(error.message);
        }
        if (!blob.bytes.equals(bytes)) {
          throw rewireError(
            manifest +
              " in the working tree differs from " +
              blob.commit +
              ":" +
              manifest +
              "; refusing",
          );
        }
      }
      return bytes;
    },
  };
}

// `root` is the checkout the contract, test pins and README are rewritten in;
// its own lock-fingerprint.mjs computes every fingerprint. Async: the checkout
// helper is imported, and the artifact-less fingerprint-helper reason awaits.
// `stageIo` is openOutputSession's test seam.
export async function main(argv = process.argv.slice(2), root = ".", stageIo = undefined) {
  const args = parseArgs(argv);
  if (Object.hasOwn(args, "apply")) return applyStage(args.apply, root);
  if (args.reason === FINGERPRINT_HELPER_REASON) {
    return runFingerprintHelperRepin(args["base-ref"], root);
  }
  const at = (path) => join(root, path);
  const session = openOutputSession(root, args.stage, stageIo);
  const measurements = {};
  const normalized = {};
  for (const mode of MODES) {
    const modeDir = join(args.artifact, "real-" + mode);
    if (!existsSync(join(modeDir, "measurement.json"))) {
      throw new Error("artifact is missing real-" + mode + "/measurement.json");
    }
    const { sidecar } = normalizeMeasurementDirectory(modeDir);
    if (SOURCE_BOUND_REASONS.includes(args.reason)) {
      const problem = savedSidecarProblem(modeDir, sidecar);
      if (problem) throw new Error("real-" + mode + ": " + problem + "; refusing");
    }
    normalized[mode] = sidecar.results;
    measurements[mode] = JSON.parse(readFileSync(join(modeDir, "measurement.json"), "utf8"));
  }
  const provenance = measurements.workspace.provenance;
  for (const mode of MODES) assertLinuxX64Provenance(measurements[mode].provenance);
  if (Object.hasOwn(args, "source-sha")) {
    for (const mode of MODES) {
      const measured = measurements[mode].provenance.sourceSha;
      if (measured !== args["source-sha"]) {
        throw new Error(
          mode + " artifact measured source " + String(measured) + ", not --source-sha",
        );
      }
    }
  }
  const contractBytes = readFileSync(at(CONTRACT));
  const contract = JSON.parse(contractBytes.toString("utf8"));
  const helper = await loadCheckoutHelper(root);
  const checkout = readCheckoutIdentity(contract, root, helper);
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
    "cargo-producer-lock": "Cargo producer-lock transition run at v" + version,
    "dependency-graph": "Dependency-graph re-pin run at v" + version,
    "fingerprint-policy": "Fingerprint-policy re-pin run at v" + version,
    "cargo-internal-rewire": "Cargo internal-rewire transition run at v" + version,
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
      checkoutHelper: checkout.checkoutHelper,
      ...(args.reason === CARGO_PRODUCER_LOCK_REASON
        ? { cargoLock: readCargoLockEvidence(args, root) }
        : {}),
      ...(args.reason === CARGO_INTERNAL_REWIRE_REASON
        ? { cargoRewire: readCargoRewireEvidence(args, root, LINUX_PRODUCER_TARGET) }
        : {}),
      ...(REPINNED_FROM_REASONS.includes(args.reason)
        ? {
            previousContract: previousContractReference(
              root,
              at(CONTRACT),
              contractBytes,
              args["previous-contract-ref"],
            ),
          }
        : {}),
    },
    args.reason,
  );
  // Every output is rendered before the first write.
  const outputs = [
    { path: CONTRACT, bytes: JSON.stringify(result.contract, null, 2) + "\n" },
    {
      path: TEST_FILE,
      bytes: rewriteTestPins(session.read(TEST_FILE).toString("utf8"), result.contract),
    },
    {
      path: README,
      bytes:
        session.read(README).toString("utf8") +
        readmeSection(result.contract, result.gzipEvidence, result.previousVersion, args.reason),
    },
  ];
  if (INVENTORY_REASONS.includes(args.reason)) {
    outputs.push({
      path: INVENTORY_FILE,
      bytes: regenerateInventory(root, result.contract.dependencyFingerprint, helper, args.reason),
    });
  }
  if (args.reason === FINGERPRINT_POLICY_REASON) {
    outputs.push({
      path: POLICY_DOC,
      bytes: rewritePolicyIdentity(
        session.read(POLICY_DOC).toString("utf8"),
        result.contract.dependencyFingerprint,
      ),
    });
  }
  session.commit(outputs, {
    reason: args.reason,
    platform: "linux-x64",
    sourceSha: provenance.sourceSha,
    headSha: args["head-sha"],
    runId: Number(args["run-id"]),
    artifactId: args["artifact-id"] === undefined ? null : Number(args["artifact-id"]),
    ...stageRunIdentity(args.reason, contract, result.contract),
  });
  console.log(
    (args.stage === undefined ? "re-pinned" : "staged") +
      " Linux island-size contract to v" +
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
    await main();
  } catch (error) {
    console.error("rebaseline-island-size-linux: " + error.message);
    process.exit(1);
  }
}
