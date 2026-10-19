// Local evidence identity only. Measurements still pass the existing Darwin
// artifact validators. No commands in a receipt are executed; no signing trust
// is inferred from an operator-supplied digest.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { lstatSync, readFileSync, readdirSync, realpathSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";

import {
  DECISION_SHA256,
  OLD_LOCK_SHA256,
  NEW_LOCK_SHA256,
} from "./rustls-security-transition.mjs";

const fail = (message) => {
  throw new Error("local Darwin evidence: " + message);
};

export function assertLocalSecurityTransition(transition) {
  if (
    transition?.oldSha !== OLD_LOCK_SHA256 ||
    transition?.newSha !== NEW_LOCK_SHA256 ||
    transition?.review?.id !== "rustls-security-4314" ||
    transition?.review?.manifestSha256 !== DECISION_SHA256
  )
    fail("only the exact rustls-security-4314 transition supports local evidence");
}

// The only reasons that may stage owner-attested local evidence, each for its
// own reviewed purpose: #4311 internal Cargo rewires (evidence.cargoRewire) and
// the #4314 rustls security lock transition (evidence.cargoTransition). Adding
// a reason here is a provenance-policy decision, not a refactor.
export const LOCAL_EVIDENCE_PROFILES = Object.freeze({
  "cargo-internal-rewire": Object.freeze({
    transitionKey: "cargoRewire",
    requiresReviewedTransition: false,
    assertContract(contract) {
      if (
        !contract.baselineEvidence?.cargoRewire ||
        typeof contract.baselineEvidence.cargoRewire !== "object"
      )
        fail("cargo-internal-rewire evidence lacks its rewire proof");
    },
  }),
  "cargo-producer-lock": Object.freeze({
    transitionKey: "cargoTransition",
    requiresReviewedTransition: true,
    assertContract(contract) {
      assertLocalSecurityTransition(contract.baselineEvidence?.cargoTransition);
      if (contract.toolchain?.cargoLockSha256 !== NEW_LOCK_SHA256)
        fail("staged contract has wrong security lock");
    },
  }),
});
const TRANSITION_KEYS = ["cargoRewire", "cargoTransition"];

export function localEvidenceProfile(reason) {
  if (typeof reason !== "string" || !Object.hasOwn(LOCAL_EVIDENCE_PROFILES, reason))
    fail("reason " + JSON.stringify(reason) + " has no local evidence profile");
  return LOCAL_EVIDENCE_PROFILES[reason];
}

const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const digest = (value) => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const text = (value) =>
  typeof value === "string" &&
  value.trim().length > 0 &&
  Array.from(value).every((char) => char.charCodeAt(0) >= 32 && char.charCodeAt(0) !== 127);
function keys(value, expected) {
  return (
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...expected].sort())
  );
}
function noLinks(path) {
  const absolute = resolve(path);
  let cursor = absolute;
  for (;;) {
    if (lstatSync(cursor).isSymbolicLink()) fail("symlink in evidence path " + cursor);
    const parent = resolve(cursor, "..");
    if (parent === cursor) break;
    cursor = parent;
  }
  return absolute;
}

// Digest the complete retained directory, with stable portable relative paths.
// Empty directories do not carry evidence. Symlinks and special files fail closed.
export function localBundleDigest(directory) {
  const root = noLinks(directory);
  if (!lstatSync(root).isDirectory()) fail("bundle must be a directory");
  const inventory = [];
  function walk(dir, prefix = "") {
    for (const name of readdirSync(dir).sort()) {
      if (!text(name) || name.includes("\\")) fail("unsafe bundle entry name");
      const path = join(dir, name);
      const rel = prefix + name;
      const stat = lstatSync(path);
      if (stat.isDirectory()) walk(path, rel + "/");
      else if (stat.isFile()) inventory.push([rel, stat.size, sha(readFileSync(path))]);
      else fail("symlink or special file in bundle: " + rel);
    }
  }
  walk(root);
  if (!inventory.length) fail("empty bundle");
  return sha(JSON.stringify(inventory));
}

export function validateLocalReceipt(receipt, sourceSha) {
  if (
    !keys(receipt, [
      "schemaVersion",
      "kind",
      "operator",
      "collectedAt",
      "source",
      "platform",
      "commands",
      "bundle",
    ]) ||
    receipt.schemaVersion !== 1 ||
    receipt.kind !== "local-native-darwin"
  )
    fail("invalid receipt schema or provenance kind");
  if (!text(receipt.operator)) fail("operator is required");
  if (
    typeof receipt.collectedAt !== "string" ||
    !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(receipt.collectedAt) ||
    !Number.isFinite(Date.parse(receipt.collectedAt)) ||
    new Date(receipt.collectedAt).toISOString() !== receipt.collectedAt ||
    Date.parse(receipt.collectedAt) > Date.now()
  )
    fail("collection time must be a valid past UTC timestamp");
  if (
    !keys(receipt.source, ["commit", "clean"]) ||
    receipt.source.clean !== true ||
    !/^[a-f0-9]{40}$/.test(receipt.source.commit ?? "") ||
    receipt.source.commit !== sourceSha
  )
    fail("exact source and clean state must match");
  if (
    !keys(receipt.platform, ["os", "arch"]) ||
    receipt.platform.os !== "darwin" ||
    receipt.platform.arch !== "arm64"
  )
    fail("native Darwin arm64 is required");
  if (
    !Array.isArray(receipt.commands) ||
    !receipt.commands.length ||
    !receipt.commands.every((argv) => Array.isArray(argv) && argv.length && argv.every(text))
  )
    fail("command argv records are required");
  if (
    !keys(receipt.bundle, ["location", "sha256"]) ||
    !text(receipt.bundle.location) ||
    !digest(receipt.bundle.sha256)
  )
    fail("retained bundle location and digest are required");
  return receipt;
}

export function verifyLocalCheckout(root, sourceSha, label = "measured source") {
  const git = (...args) =>
    execFileSync("git", ["--no-replace-objects", ...args], {
      cwd: root,
      encoding: "utf8",
    }).trim();
  if (git("rev-parse", "HEAD") !== sourceSha)
    fail(label + " checkout HEAD differs from its recorded commit");
  if (git("status", "--porcelain", "--untracked-files=normal"))
    fail(label + " checkout must be clean");
}

export function readLocalEvidence(receiptPath, bundlePath, expectedSha, sourceSha) {
  if (!digest(expectedSha)) fail("explicit receipt SHA-256 is required");
  const receiptFile = noLinks(receiptPath);
  const bundle = noLinks(bundlePath);
  if (!lstatSync(receiptFile).isFile()) fail("receipt must be a regular file");
  const rel = relative(bundle, receiptFile);
  if (!isAbsolute(rel) && rel !== ".." && !rel.startsWith(".." + sep))
    fail("receipt must be outside its bundle");
  const bytes = readFileSync(receiptFile);
  if (sha(bytes) !== expectedSha) fail("receipt digest mismatch");
  let parsed;
  try {
    parsed = JSON.parse(bytes.toString("utf8"));
  } catch {
    fail("receipt is not valid JSON");
  }
  const receipt = validateLocalReceipt(parsed, sourceSha);
  if (localBundleDigest(bundle) !== receipt.bundle.sha256) fail("bundle digest mismatch");
  return {
    receipt,
    sha256: expectedSha,
    receiptPath: realpathSync(receiptFile),
    bundlePath: realpathSync(bundle),
  };
}

// Called again when a stage is extended or applied. Retained evidence must
// remain available and unchanged; missing local provenance cannot become CI.
// Profile and shape checks run before any file or git access.
export function verifyLocalStageEvidence(run, contract, root) {
  const evidence = contract?.baselineEvidence;
  const local = run.localEvidence;
  if (evidence?.event !== "local-native" && !local) return;
  if (
    !Object.hasOwn(LOCAL_EVIDENCE_PROFILES, run.reason) ||
    run.platform !== "darwin-arm64" ||
    evidence?.event !== "local-native" ||
    !local ||
    ["runId", "jobId", "artifactId", "artifactExpiresAt", "artifact"].some((key) =>
      Object.hasOwn(run, key),
    ) ||
    ["runId", "jobId", "artifactId", "artifactExpiresAt", "artifact"].some((key) =>
      Object.hasOwn(evidence, key),
    )
  )
    fail("mixed or missing staged local provenance");
  const profile = LOCAL_EVIDENCE_PROFILES[run.reason];
  if (TRANSITION_KEYS.some((key) => key !== profile.transitionKey && Object.hasOwn(evidence, key)))
    fail("staged evidence carries another profile's transition");
  profile.assertContract(contract);
  if (
    !keys(local, ["receiptPath", "bundlePath", "sha256"]) ||
    !isAbsolute(local.receiptPath) ||
    !isAbsolute(local.bundlePath)
  )
    fail("invalid staged local binding");
  if (
    !keys(run.stagingTool, ["root", "commit"]) ||
    !isAbsolute(run.stagingTool.root) ||
    !/^[a-f0-9]{40}$/.test(run.stagingTool.commit ?? "") ||
    JSON.stringify(evidence.stagingTool) !== JSON.stringify({ commit: run.stagingTool.commit })
  )
    fail("missing or inconsistent tooling identity");
  const checked = readLocalEvidence(
    local.receiptPath,
    local.bundlePath,
    local.sha256,
    run.sourceSha,
  );
  if (local.receiptPath !== checked.receiptPath || local.bundlePath !== checked.bundlePath)
    fail("staged evidence paths must be canonical");
  if (
    JSON.stringify(evidence.localReceipt) !==
      JSON.stringify({ sha256: checked.sha256, receipt: checked.receipt }) ||
    contract.baselineSourceSha !== run.sourceSha ||
    evidence.sourceSha !== run.sourceSha
  )
    fail("staged contract and receipt disagree");
  verifyLocalCheckout(run.stagingTool.root, run.stagingTool.commit, "tooling");
  verifyLocalCheckout(root, run.sourceSha, "measured source");
}
