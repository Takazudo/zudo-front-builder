#!/usr/bin/env node
// Release-time Linux x64 island-size rebaseline (used by /l-make-release Step 4a).
//
// The island runtime embeds the release version, so every version bump moves a
// few gzip totals by a handful of bytes while every raw total stays identical.
// This script records a CI run's measured totals as the new exact ceilings and
// refuses anything that is not that shape: a raw change means real product
// bytes moved and needs a reviewed decision, not a release rebaseline.

import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MODES = ["workspace", "packed"];
const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
const README = "research/v3-island-size/README.md";
const TEST_FILE = "scripts/__tests__/island-size-budget.test.mjs";
const TEST_ANCHOR = 'it("records the reviewed Linux x64 ceilings with zero allowance"';

export function measuredTotals(measurementsByMode) {
  const totals = {};
  for (const mode of MODES) {
    totals[mode] = {};
    for (const [fixture, result] of Object.entries(measurementsByMode[mode].results)) {
      totals[mode][fixture] = { raw: result.shipped.raw, gzip: result.shipped.gzip };
    }
  }
  return totals;
}

export function diffCeilings(contract, totals) {
  const missingFixtures = [];
  const rawChanges = [];
  const gzipChanges = [];
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
      } else if (before.gzip !== after.gzip) {
        gzipChanges.push({ mode, fixture, delta: after.gzip - before.gzip });
      }
    }
  }
  return { missingFixtures, rawChanges, gzipChanges };
}

function signed(n) {
  return n > 0 ? "+" + n : String(n);
}

function joinNames(names) {
  return names.length < 2 ? names.join("") : names.slice(0, -1).join(", ") + " and " + names.at(-1);
}

export function describeGzipChanges(gzipChanges) {
  const parts = [];
  for (const mode of MODES) {
    const byDelta = new Map();
    for (const change of gzipChanges.filter((c) => c.mode === mode)) {
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

function deltaRange(gzipChanges) {
  const deltas = gzipChanges.map((c) => c.delta);
  return signed(Math.min(...deltas)) + " to " + signed(Math.max(...deltas));
}

export function rebaselineContract(contract, totals, evidence) {
  const { missingFixtures, rawChanges, gzipChanges } = diffCeilings(contract, totals);
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
        "); this is not a version-stamp rebaseline — " +
        "record a reviewed decision instead",
    );
  }
  const previousVersion = contract.toolchain.packageVersion;
  const summary =
    gzipChanges.length === 0
      ? "no gzip total changed"
      : gzipChanges.length + " gzip totals shifted by " + deltaRange(gzipChanges) + " bytes";
  const next = structuredClone(contract);
  next.outcome = "no-product-code-change";
  next.baselineSourceSha = evidence.headSha;
  next.toolchain.packageVersion = evidence.version;
  next.toolchain.pnpmLockSha256 = evidence.pnpmLockSha256;
  next.platformStatus =
    "Linux x64 has exact zero-allowance v" +
    evidence.version +
    " ceilings measured in health run " +
    evidence.runId +
    " at " +
    sourceText(evidence.sourceSha, evidence.headSha) +
    ". All 16 raw totals are unchanged from the v" +
    previousVersion +
    " contract; " +
    summary +
    " after the release version stamp. Workspace and " +
    "packed eight-fixture matrices each have two byte-identical full-dist inventories. Other " +
    "platforms and toolchains are not covered.";
  next.baselineEvidence = {
    event: "ci",
    runId: evidence.runId,
    jobId: evidence.jobId,
    sourceSha: evidence.sourceSha,
    note:
      evidence.runLabel +
      " at " +
      sourceText(evidence.sourceSha, evidence.headSha) +
      ". Complete Linux x64 workspace and packed eight-fixture matrices, each " +
      "with two identical full-dist inventories. Raw totals did not change from the v" +
      previousVersion +
      " contract; " +
      (gzipChanges.length === 0
        ? "no gzip total changed"
        : "gzip deltas range from " +
          deltaRange(gzipChanges) +
          " bytes across " +
          gzipChanges.length +
          " rows") +
      ". Ceilings equal the artifact measurements with zero allowance; earlier ceilings remain in git history.",
  };
  for (const mode of MODES) {
    for (const fixture of Object.keys(next.ceilings[mode])) {
      next.ceilings[mode][fixture] = { ...totals[mode][fixture] };
    }
  }
  return { contract: next, gzipChanges, previousVersion };
}

function fixtureKey(fixture) {
  return /^[a-z]+$/.test(fixture) ? fixture : JSON.stringify(fixture);
}

export function rewriteTestPins(source, contract) {
  const start = source.indexOf(TEST_ANCHOR);
  if (start < 0) throw new Error("Linux ceiling test not found in " + TEST_FILE);
  const end = source.indexOf("\n  });", start);
  let block = source.slice(start, end);
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
        "      " + fixtureKey(fixture) + ": { raw: " + c.raw + ", gzip: " + c.gzip + " },",
    );
    block =
      block.slice(0, blockStart + marker.length) +
      "\n" +
      lines.join("\n") +
      "\n    " +
      block.slice(blockEnd);
  }
  return source.slice(0, start) + block + source.slice(end);
}

export function readmeSection(contract, gzipChanges, previousVersion) {
  const { runId, jobId, sourceSha } = contract.baselineEvidence;
  const moved =
    gzipChanges.length === 0
      ? "left every raw and gzip total unchanged"
      : "left every raw total unchanged and moved " +
        gzipChanges.length +
        " gzip totals by " +
        deltaRange(gzipChanges) +
        " bytes (" +
        describeGzipChanges(gzipChanges) +
        ")";
  return (
    "\n## v" +
    contract.toolchain.packageVersion +
    " release baseline\n\n" +
    "The Linux contract was rebaselined for the v" +
    contract.toolchain.packageVersion +
    " release from [health run " +
    runId +
    "](https://github.com/Takazudo/zudo-front-builder/actions/runs/" +
    runId +
    ") (job " +
    jobId +
    ") at " +
    sourceText(sourceSha, contract.baselineSourceSha).replace(/([0-9a-f]{40})/g, "`$1`") +
    ". The version bump " +
    moved +
    ". The retained `island-size-linux-x64` artifact contains all eight fixtures in both modes with two " +
    "identical full-dist inventories per case. `decision-linux-x64.json` records those measured totals " +
    "as exact ceilings with zero allowance and the bump's lockfile hash; the v" +
    previousVersion +
    " ceilings remain in git history. The Darwin arm64 contract is unchanged by this release and needs " +
    "its separate native remeasurement through `island-size-darwin.yml`.\n"
  );
}

export function parseArgs(argv) {
  const values = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined)
      throw new Error("bad argument: " + argv[i]);
    values[argv[i].slice(2)] = argv[i + 1];
  }
  for (const required of ["artifact", "run-id", "job-id", "head-sha"]) {
    if (!values[required]) throw new Error("required --" + required);
  }
  for (const id of ["run-id", "job-id"]) {
    if (!/^[1-9][0-9]*$/.test(values[id]))
      throw new Error("--" + id + " must be a positive integer");
  }
  if (!/^[0-9a-f]{40}$/.test(values["head-sha"])) {
    throw new Error("--head-sha must be a full 40-character commit SHA");
  }
  return values;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const measurements = {};
  for (const mode of MODES) {
    measurements[mode] = JSON.parse(
      readFileSync(join(args.artifact, "real-" + mode, "measurement.json"), "utf8"),
    );
  }
  const provenance = measurements.workspace.provenance;
  const version = JSON.parse(readFileSync("packages/zfb/package.json", "utf8")).version;
  const pnpmLockSha256 = createHash("sha256").update(readFileSync("pnpm-lock.yaml")).digest("hex");
  for (const mode of MODES) {
    const p = measurements[mode].provenance;
    if (p.sourceSha !== provenance.sourceSha)
      throw new Error("workspace and packed source SHAs differ");
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
  const result = rebaselineContract(contract, measuredTotals(measurements), {
    version,
    pnpmLockSha256,
    sourceSha: provenance.sourceSha,
    headSha: args["head-sha"],
    runId: Number(args["run-id"]),
    jobId: Number(args["job-id"]),
    runLabel: args.label ?? "Release probe run for v" + version,
  });
  writeFileSync(CONTRACT, JSON.stringify(result.contract, null, 2) + "\n");
  writeFileSync(TEST_FILE, rewriteTestPins(readFileSync(TEST_FILE, "utf8"), result.contract));
  appendFileSync(
    README,
    readmeSection(result.contract, result.gzipChanges, result.previousVersion),
  );
  console.log(
    "rebaselined Linux island-size contract to v" +
      version +
      ": " +
      (describeGzipChanges(result.gzipChanges) || "no gzip change"),
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
