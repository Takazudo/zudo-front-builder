#!/usr/bin/env node
/**
 * verify-npm-provenance-expectations.mjs — post-publish check that every
 * package version carries exactly the npm provenance the publish helper
 * intended.
 *
 * scripts/publish-npm-packages.sh writes one TSV line per package to
 * ZFB_PROVENANCE_EXPECTATIONS_FILE: `<name>\t<version>\t<attested|none>\t<outcome>`.
 * For each line this reads `npm view <name>@<version> dist.attestations --json`
 * and fails when an expected attestation is missing or an unexpected one exists.
 *
 * Registry states are kept apart, never collapsed into "absent":
 *   - version not visible yet (E404/ETARGET right after publish) → retried
 *     (read-after-write lag), and reported as "never became visible";
 *   - transient network/registry error → retried, reported as "could not verify";
 *   - auth error (E401/E403/ENEEDAUTH) on a public read → not retried, reported;
 *   - empty output with exit 0 → the version exists and has no attestations.
 * An attestation that is present but unexpected cannot disappear, so it fails at
 * once; an expected one that is absent is retried within the budget before
 * failing as a mismatch.
 *
 * Usage: node scripts/verify-npm-provenance-expectations.mjs --expectations <file>
 *   env NPM_PROVENANCE_VERIFY_ATTEMPTS (default 6), NPM_PROVENANCE_VERIFY_DELAY_MS
 *   (default 5000, doubled per retry).
 */

import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

/** SLSA predicate npm records for GitHub Actions provenance (registry, 6.0.1). */
export const SLSA_PROVENANCE_V1 = "https://slsa.dev/provenance/v1";

export function parseExpectations(text) {
  const rows = [];
  for (const [index, raw] of String(text ?? "")
    .split(/\r?\n/)
    .entries()) {
    if (raw.trim() === "") continue;
    const [name, version, expected, outcome = ""] = raw.split("\t");
    if (!name || !version || !["attested", "none"].includes(expected)) {
      throw new Error(`malformed expectation line ${index + 1}: ${JSON.stringify(raw)}`);
    }
    rows.push({ name, version, expected, outcome });
  }
  return rows;
}

/** Classify an npm failure message. */
export function classifyNpmError(message) {
  const text = String(message ?? "");
  if (/\bE(401|403|NEEDAUTH|OTP)\b/.test(text)) return "auth";
  if (/\bE404\b|\bETARGET\b|notarget|No match found for version/i.test(text)) return "not-visible";
  return "transient";
}

/**
 * One observation of the registry: { state: "attested" | "none" | "other" |
 * "not-visible" | "transient" | "auth", detail }.
 */
export async function observeAttestation(name, version, npm) {
  let stdout;
  try {
    stdout = await npm([
      "view",
      `${name}@${version}`,
      "dist.attestations",
      "--json",
      "--prefer-online",
    ]);
  } catch (error) {
    const detail = `${error.message ?? ""}\n${error.stdout ?? ""}`.trim();
    return { state: classifyNpmError(detail), detail };
  }
  const trimmed = String(stdout ?? "").trim();
  if (trimmed === "") return { state: "none", detail: "no dist.attestations" };
  let parsed;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return { state: "transient", detail: `unparseable npm view output: ${trimmed.slice(0, 200)}` };
  }
  // npm 12 (the publish job's pinned CLI) prints `npm view <spec> <field> --json` as a
  // one-element array; older CLIs print the bare object.
  const records = Array.isArray(parsed) ? parsed : [parsed];
  if (
    records.length > 0 &&
    records.every((record) => record?.provenance?.predicateType === SLSA_PROVENANCE_V1)
  ) {
    return { state: "attested", detail: SLSA_PROVENANCE_V1 };
  }
  return { state: "other", detail: `unexpected attestation shape: ${trimmed.slice(0, 200)}` };
}

async function defaultNpm(args) {
  const { stdout } = await execFileAsync("npm", args, { encoding: "utf8" });
  return stdout;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Verify one row; returns { ok, verdict, detail }. */
export async function verifyRow(
  row,
  { npm = defaultNpm, wait = sleep, attempts = 6, delayMs = 5000 } = {},
) {
  let delay = delayMs;
  let last = { state: "transient", detail: "not attempted" };
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    last = await observeAttestation(row.name, row.version, npm);
    if (last.state === row.expected) return { ok: true, verdict: "match", detail: last.detail };
    if (last.state === "auth") return { ok: false, verdict: "auth-error", detail: last.detail };
    if (last.state === "other") return { ok: false, verdict: "mismatch", detail: last.detail };
    if (last.state === "attested" && row.expected === "none") {
      return {
        ok: false,
        verdict: "mismatch",
        detail: "attestation present but none was expected",
      };
    }
    if (attempt < attempts) {
      await wait(delay);
      delay *= 2;
    }
  }
  if (last.state === "none") {
    return {
      ok: false,
      verdict: "mismatch",
      detail: "expected an SLSA provenance attestation; none recorded",
    };
  }
  if (last.state === "not-visible") {
    return {
      ok: false,
      verdict: "not-visible",
      detail: `version never became visible: ${last.detail}`,
    };
  }
  return { ok: false, verdict: "registry-error", detail: `could not verify: ${last.detail}` };
}

export async function verifyExpectations(rows, options = {}) {
  const { log = console.log } = options;
  const results = [];
  for (const row of rows) {
    const result = await verifyRow(row, options);
    results.push({ ...row, ...result });
    if (result.ok) {
      log(`  ✓ ${row.name}@${row.version}: provenance ${row.expected} as expected.`);
    } else {
      log(
        `::error title=npm provenance expectation (${result.verdict})::${row.name}@${row.version}: expected ${row.expected} (publish outcome: ${row.outcome || "unknown"}) — ${result.detail}`,
      );
    }
  }
  return { results, failed: results.filter((r) => !r.ok).length };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const flag = process.argv.indexOf("--expectations");
  const file = flag >= 0 ? process.argv[flag + 1] : undefined;
  if (!file) {
    console.log("::error::usage: verify-npm-provenance-expectations.mjs --expectations <file>");
    process.exit(2);
  }
  let rows;
  try {
    rows = parseExpectations(readFileSync(file, "utf8"));
  } catch (error) {
    console.log(`::error::cannot read provenance expectations: ${error.message}`);
    process.exit(1);
  }
  if (rows.length === 0) {
    console.log(
      `::error::provenance expectations file ${file} is empty — the publish helper recorded nothing.`,
    );
    process.exit(1);
  }
  const { failed } = await verifyExpectations(rows, {
    attempts: Number(process.env.NPM_PROVENANCE_VERIFY_ATTEMPTS ?? 6),
    delayMs: Number(process.env.NPM_PROVENANCE_VERIFY_DELAY_MS ?? 5000),
  });
  console.log(`Provenance expectations: ${rows.length - failed}/${rows.length} verified.`);
  process.exit(failed === 0 ? 0 : 1);
}
