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
// unchanged, and only the lock hash may move.

import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  IDENTITY_NORMALIZATION_VERSION,
  IDENTITY_NORMALIZED_METRIC,
  normalizeMeasurementDirectory,
  normalizerSha256,
} from "../research/v3-island-size/normalize-identity.mjs";

const MODES = ["workspace", "packed"];
const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
const README = "research/v3-island-size/README.md";
const TEST_FILE = "scripts/__tests__/island-size-budget.test.mjs";
const TEST_ANCHOR = 'it("records the reviewed Linux x64 ceilings with zero allowance"';

export const REASONS = Object.freeze({
  "release-bump": {
    outcome: "release-version-stamp",
    heading: "release baseline",
    requires: 2,
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
  },
  "lockfile-provenance": {
    outcome: "lockfile-provenance-repin",
    heading: "lockfile-provenance re-pin",
    requires: 2,
  },
});

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

function sameHashMap(a, b) {
  if (!a || !b) return false;
  const keys = Object.keys(a);
  return (
    keys.length === Object.keys(b).length &&
    keys.every((key) => Object.hasOwn(b, key) && a[key] === b[key])
  );
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
  if (!sameHashMap(identity.fixtureSha256, contract.fixtureSha256)) moved.push("fixtureSha256");
  if (!sameHashMap(identity.runnerSha256, contract.runnerSha256)) moved.push("runnerSha256");
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
  const normalizedContract = contract.schemaVersion === 2;
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
// reported as transfer-size evidence; never a ceiling under schemaVersion 2.
// schemaVersion 1 ceilings are actual gzip; schemaVersion 2 ceilings carry only
// gzipNormalized, so the previous actual gzip is read from the evidence a
// re-pin records in baselineEvidence.actualGzip. Null when none was recorded.
export function actualGzipEvidence(contract, totals) {
  const changes = [];
  for (const mode of MODES) {
    for (const [fixture, after] of Object.entries(totals[mode])) {
      const before =
        contract.schemaVersion === 2
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
  if (contract.schemaVersion !== policy.requires) {
    throw new Error(
      "--reason " +
        reason +
        " needs a schemaVersion " +
        policy.requires +
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
  const run =
    "[health run " +
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

function main() {
  const args = parseArgs(process.argv.slice(2));
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
  const version = JSON.parse(readFileSync("packages/zfb/package.json", "utf8")).version;
  const pnpmLockSha256 = createHash("sha256").update(readFileSync("pnpm-lock.yaml")).digest("hex");
  for (const mode of MODES) {
    const p = measurements[mode].provenance;
    if (p.sourceSha !== provenance.sourceSha)
      throw new Error("workspace and packed source SHAs differ");
    if (JSON.stringify(measuredIdentity(p)) !== JSON.stringify(measuredIdentity(provenance))) {
      throw new Error("workspace and packed identity inputs differ");
    }
    if (p.pnpmLockSha256 !== pnpmLockSha256) {
      throw new Error(mode + " artifact lockfile hash differs from this checkout's pnpm-lock.yaml");
    }
    if (p.packageVersion !== version) {
      throw new Error(
        mode + " artifact measured @takazudo/zfb " + p.packageVersion + ", not " + version,
      );
    }
  }
  const contract = JSON.parse(readFileSync(CONTRACT, "utf8"));
  const defaultLabel = {
    "release-bump": "Release probe run for v" + version,
    "metric-migration":
      "Metric migration replayed from the retained artifact of health run " + args["run-id"],
    "runtime-identity": "Runtime-identity re-pin run at v" + version,
    "lockfile-provenance": "Consumer lockfile re-pin run at v" + version,
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
    },
    args.reason,
  );
  writeFileSync(CONTRACT, JSON.stringify(result.contract, null, 2) + "\n");
  writeFileSync(TEST_FILE, rewriteTestPins(readFileSync(TEST_FILE, "utf8"), result.contract));
  appendFileSync(
    README,
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
