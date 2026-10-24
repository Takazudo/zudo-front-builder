#!/usr/bin/env node
// Collection eligibility only, not re-pin admission. Keep the budget check's
// nonzero exit and output intact; a complete old-Cargo-pin-only rejection is
// retained for the separately reviewed, paired platform activation procedure.
import { execFileSync, spawnSync } from "node:child_process";
import { appendFileSync, readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath } from "node:url";
// Compare canonical paths: a symlinked invocation path (macOS /var -> /private/var) must still run main() (#4465).
import { realpathSync as realpathSyncForMain } from "node:fs";
import { normalizeMeasurementDirectory } from "../research/v3-island-size/normalize-identity.mjs";

const modes = ["workspace", "packed"];
const oldPinErrors = [
  "local Cargo.lock: SHA-256 differs from the reviewed contract",
  "workspace provenance: Cargo lockfile differs from the contract",
  "packed provenance: Cargo lockfile differs from the contract",
];

export function budgetAllowsRetention(result) {
  if (result.error || result.signal) return false;
  if (result.status === 0) return true;
  if (result.status !== 1) return false;
  // Exact complete diagnostic, not a substring allowlist that can hide an
  // additional exception or an unexpected checker failure.
  const expected = ["Island size budget rejected:", ...oldPinErrors.map((s) => "- " + s)];
  return isDeepStrictEqual(result.stderr.trim().split(/\r?\n/), expected);
}

export function assertCompletePair(directory, contract, sourceSha) {
  if (!/^[a-f0-9]{40}$/.test(sourceSha)) throw new Error("expected exact checkout source SHA");
  for (const key of ["os", "arch"]) {
    if (typeof contract.platform?.[key] !== "string" || !contract.platform[key].trim()) {
      throw new Error("missing or invalid contract platform");
    }
  }
  for (const mode of modes) {
    const dir = resolve(directory, "real-" + mode);
    const measurement = JSON.parse(readFileSync(resolve(dir, "measurement.json"), "utf8"));
    if (measurement.provenance.sourceSha !== sourceSha) {
      throw new Error(mode + ": stale measured source SHA");
    }
    if (
      measurement.provenance.platform?.os !== contract.platform.os ||
      measurement.provenance.platform?.arch !== contract.platform.arch
    ) {
      throw new Error(mode + ": measured platform differs from the selected contract");
    }
    const fixtures = Object.keys(contract.ceilings[mode]).sort();
    const expectedEntries = [
      ...fixtures,
      "measurement.json",
      "measurement.normalized.json",
      "report.md",
    ].sort();
    if (
      !isDeepStrictEqual(readdirSync(dir).sort(), expectedEntries) ||
      !isDeepStrictEqual(Object.keys(measurement.results).sort(), fixtures)
    ) {
      throw new Error(mode + ": incomplete measurement rows or unexpected outputs");
    }
    const saved = JSON.parse(readFileSync(resolve(dir, "measurement.normalized.json"), "utf8"));
    const { sidecar } = normalizeMeasurementDirectory(dir);
    if (!isDeepStrictEqual(saved, JSON.parse(JSON.stringify(sidecar)))) {
      throw new Error(mode + ": stale or altered normalized sidecar");
    }
    for (const fixture of fixtures) {
      for (const metric of ["raw", "gzipNormalized"]) {
        if (sidecar.results[fixture].shipped[metric] !== contract.ceilings[mode][fixture][metric]) {
          throw new Error(
            mode +
              "/" +
              fixture +
              ": changed " +
              metric +
              " total is not activation evidence (including decreases)",
          );
        }
      }
    }
  }
}

export function runBudgetEvidence({
  root,
  sourceSha,
  output,
  check,
  log = process.stdout,
  errors = process.stderr,
}) {
  const directory = resolve(root, "target/island-size");
  const contractPath = resolve(root, "research/v3-island-size/decision-linux-x64.json");
  const result = check([
    resolve(root, "research/v3-island-size/check-budget.mjs"),
    "--workspace",
    resolve(directory, "real-workspace/measurement.json"),
    "--packed",
    resolve(directory, "real-packed/measurement.json"),
    "--contract",
    contractPath,
  ]);
  if (result.stdout) log.write(result.stdout);
  if (result.stderr) errors.write(result.stderr);
  if (!budgetAllowsRetention(result)) return result.status || 1;
  try {
    assertCompletePair(directory, JSON.parse(readFileSync(contractPath, "utf8")), sourceSha);
  } catch (error) {
    errors.write("Island size evidence withheld: " + error.message + "\n");
    return result.status || 1;
  }
  if (output) appendFileSync(output, "complete=true\n");
  log.write(
    "Complete unchanged-total evidence retained; budget verdict remains " + result.status + ".\n",
  );
  return result.status;
}

if (
  process.argv[1] &&
  realpathSyncForMain(process.argv[1]) === realpathSyncForMain(fileURLToPath(import.meta.url))
) {
  try {
    const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
    process.exitCode = runBudgetEvidence({
      root,
      sourceSha: execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim(),
      output: process.env.GITHUB_OUTPUT,
      check: (args) =>
        spawnSync(process.execPath, args, {
          cwd: root,
          encoding: "utf8",
          maxBuffer: 16 * 1024 * 1024,
        }),
    });
  } catch (error) {
    console.error(error.stack ?? String(error));
    process.exitCode = 1;
  }
}
