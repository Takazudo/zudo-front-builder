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
// additive Cargo.lock move from a fresh artifact: it binds the old and new lock
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
// `baselineSourceSha` names the measured source; `contract.commit` names the
// commit that stores the previous contract file. They are different commits.
//
// No file is written before every validation and every rendered output
// succeeds; there is no force or ignore-provenance option of any kind.

import { execFileSync, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { appendFileSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";

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
});

export const FINGERPRINT_HELPER_REASON = "fingerprint-helper";
export const CARGO_PRODUCER_LOCK_REASON = "cargo-producer-lock";

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
]);
// Corroboration only: the old lock's source of truth is `--old-cargo-ref`.
export const OPTIONAL_CARGO_ARGS = Object.freeze(["old-cargo-lock"]);
export const OLD_CARGO_LOCK_PATH = "Cargo.lock";
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
// cargo-producer-lock alone, which requires all but OPTIONAL_CARGO_ARGS.
export function checkCargoProducerLockArgs(values) {
  if (values.reason !== CARGO_PRODUCER_LOCK_REASON) {
    const stray = CARGO_ARGS.filter((key) => Object.hasOwn(values, key));
    if (stray.length > 0) {
      throw new Error("--" + stray[0] + " is only for --reason " + CARGO_PRODUCER_LOCK_REASON);
    }
    return;
  }
  for (const key of CARGO_ARGS) {
    if (!OPTIONAL_CARGO_ARGS.includes(key) && !values[key]) {
      throw new Error("required --" + key);
    }
  }
  for (const key of ["expected-old-cargo-sha", "expected-new-cargo-sha"]) {
    if (!/^[0-9a-f]{64}$/.test(values[key])) {
      throw new Error("--" + key + " must be a 64-character SHA-256");
    }
  }
  parseOldCargoRef(values["old-cargo-ref"]);
  parseAddedPackages(values["added-packages"]);
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

// Proves one reviewed additive Cargo.lock transition and returns the
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
  const diff = diffCargoLocks(decodeLock(oldText), decodeLock(newText));
  const allowlist = [...added].sort();
  const found = [...diff.added].sort();
  if (diff.changed.length > 0) problems.push("changed records " + diff.changed.join(", "));
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
  const moved = contractDiffKeys(before, after).filter(
    (key) =>
      !CARGO_PRODUCER_LOCK_MOVEMENT.some(
        (allowed) => key === allowed || key.startsWith(allowed + "."),
      ),
  );
  if (moved.length > 0) throw cargoError("would move " + moved.join(", ") + "; refusing");
}

// cargo-producer-lock may move only Cargo.lock: everything else the contract
// pins must equal the measured artifact and this checkout.
function assertCargoOnlyMovement(contract, evidence) {
  const identity = evidence.identity;
  if (!identity) throw cargoError("needs the artifact's measured identity inputs");
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
    throw cargoError(
      "allows only Cargo.lock to move, but " +
        moved.join(", ") +
        " differ from the contract; use the reason that matches the change",
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

export function cargoTransitionMarkdown(transition) {
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
  if (contract.schemaVersion === 3) assertFingerprintUnchanged(contract, evidence, reason);
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
      " after a reviewed additive Cargo.lock transition (" +
      cargoTransitionText(cargoTransition) +
      "). The producer version, fixtures, runners, pnpm-lock.yaml and its dependency fingerprint, toolchain and normalizer are unchanged, and every raw and gzipNormalized total is unchanged; only the Cargo.lock hash moved. Equal totals alone do not prove byte-identical product code. " +
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
              ? "Reviewed additive Cargo.lock transition at unchanged v" +
                previousVersion +
                " (" +
                cargoTransitionText(cargoTransition) +
                "); every other reviewed identity input is unchanged; raw and gzipNormalized totals did not change; " +
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
  const runKind =
    reason === "fingerprint-migration" || reason === CARGO_PRODUCER_LOCK_REASON
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
  } else if (reason === CARGO_PRODUCER_LOCK_REASON) {
    body =
      "The Linux contract recorded a reviewed additive `Cargo.lock` transition at v" +
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
export const FINGERPRINT_HELPER_FILE = SIZE_DIR + "/lock-fingerprint.mjs";
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
  return execFileSync("git", args, gitOptions(root));
}

// Like git() but returns {status, stdout, stderr} so a warning on success is visible.
function runGit(root, args) {
  const result = spawnSync("git", args, gitOptions(root));
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
    const moved = contractDiffKeys(contract, next);
    if (moved.some((key) => !FINGERPRINT_HELPER_MOVEMENT.includes(key))) {
      throw helperRepinError("would move " + moved.join(", ") + "; refusing");
    }
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
  // The helper's CLI guard compares argv[1] with its resolved module path.
  const helperPath = realpathSync(at(FINGERPRINT_HELPER_FILE));
  const explain = execFileSync(process.execPath, [helperPath, "--explain"], {
    maxBuffer: 64 * 1024 * 1024,
  });
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
  if (checkFingerprintHelperArgs(values)) return values;
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
  checkCargoProducerLockArgs(values);
  checkPreviousContractRefArgs(values);
  return values;
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
  };
}

// `root` is the checkout the contract, test pins and README are rewritten in.
// The artifact-less fingerprint-helper reason returns a promise.
export function main(argv = process.argv.slice(2), root = ".") {
  const args = parseArgs(argv);
  if (args.reason === FINGERPRINT_HELPER_REASON) {
    return runFingerprintHelperRepin(args["base-ref"], root);
  }
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
  const contractBytes = readFileSync(at(CONTRACT));
  const contract = JSON.parse(contractBytes.toString("utf8"));
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
    "cargo-producer-lock": "Cargo producer-lock transition run at v" + version,
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
      ...(args.reason === CARGO_PRODUCER_LOCK_REASON
        ? { cargoLock: readCargoLockEvidence(args, root) }
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
  const contractText = JSON.stringify(result.contract, null, 2) + "\n";
  const testText = rewriteTestPins(readFileSync(at(TEST_FILE), "utf8"), result.contract);
  const readmeText = readmeSection(
    result.contract,
    result.gzipEvidence,
    result.previousVersion,
    args.reason,
  );
  writeFileSync(at(CONTRACT), contractText);
  writeFileSync(at(TEST_FILE), testText);
  appendFileSync(at(README), readmeText);
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
    await main();
  } catch (error) {
    console.error("rebaseline-island-size-linux: " + error.message);
    process.exit(1);
  }
}
