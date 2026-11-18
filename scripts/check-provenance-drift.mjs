#!/usr/bin/env node
//
// check-provenance-drift.mjs — catch published packages that lose their npm
// provenance attestation, or never acquire one under the all-published policy.
//
// WHY THIS EXISTS (incident: v2.12.0, issue #2623)
// ---------------------------------------------------------------------------
// npm records a provenance attestation per published version, and versions are
// immutable. A version published WITHOUT an attestation after earlier versions
// carried one is therefore a PERMANENT trust downgrade: a consumer running
// pnpm's opt-in `trustPolicy: no-downgrade` cannot install it at all, and gets
// ERR_PNPM_TRUST_DOWNGRADE ("possible package takeover").
//
// v2.12.0 shipped exactly that — it went out through the main-based recovery
// path, which cannot attach provenance — and nothing in CI noticed. It surfaced
// only when a consumer hit it, because the breakage is invisible from inside the
// repo: the packages publish fine, the release goes green, and (below pnpm 11.2)
// even a frozen-lockfile install keeps working. This check is the missing
// registry-side guard; it would have caught v2.12.0 within a week.
//
// THE CENTRAL DESIGN DECISION: regressions and standing absence fail
// ---------------------------------------------------------------------------
// The all-published policy now makes a package that never carried an attestation
// a failure too. The temporary grandfather clause for the known legacy state
// from issue #2625 — `@takazudo/zfb-darwin-x64` while its exact `latest` dist-tag
// was `2.12.0` — expired at `2.13.0`: an attested release moved `latest` forward
// and closed that gap. Keep this history so the policy transition remains
// explicit. The only later pass for an unattested latest is an owner decision
// in release/provenance-policy.json, applied below.
//
// The resulting policy is:
//   REGRESSION  — latest has no attestation, but an EARLIER-PUBLISHED version
//                 did, and release/provenance-policy.json does not cover that
//                 package at this version. This is a real trust downgrade. FAIL.
//   ACKNOWLEDGED — same registry shape, covered by the owner's committed
//                 provenance policy at or after unattested_from_version (#4687).
//                 Darwin mac-local publishes are this case. PASS.
//   NEVER       — latest has no attestation and no earlier version did either.
//                 This violates the all-published policy. FAIL.
//   OK          — latest carries an attestation.
//
// Two details copied from pnpm so the verdict matches what a consumer sees:
//   - Ordering is by PUBLISH DATE, not semver. pnpm states this explicitly
//     ("Trust checks are based solely on publish date, not semver"), and it
//     matters here because zfb backports and prereleases interleave.
//   - Prereleases are ignored when the current `latest` is a stable version,
//     matching pnpm >= 10.24. Without this, a prerelease that happened to be
//     attested would raise a downgrade against a stable release that pnpm
//     itself would not flag.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { compareSemver, PUBLISHED_PACKAGES } from "./retire-next-dist-tag.mjs";

export const PROVENANCE_POLICY_PATH = join(
  dirname(fileURLToPath(import.meta.url)),
  "../release/provenance-policy.json",
);

const REGISTRY = "https://registry.npmjs.org";

/**
 * True when this packument version record carries a provenance attestation.
 * Exported: check-provenance-preflight.mjs reuses this rather than
 * duplicating the `dist.attestations` shape.
 */
export function hasAttestation(versionRecord) {
  return Boolean(versionRecord?.dist?.attestations);
}

/**
 * A version is a prerelease when its semver carries a `-` suffix.
 * Exported: check-provenance-preflight.mjs reuses pnpm's prerelease-exclusion
 * rule (see the file header) rather than reimplementing it.
 */
export function isPrerelease(version) {
  return String(version).includes("-");
}

/**
 * Classify one package from its registry packument.
 *
 * Pure — takes data, returns a verdict — so the tests can drive every branch
 * (including ones that are hard to reproduce against the live registry, like a
 * regression) without any network access.
 *
 * @returns {{name: string, status: "ok"|"regression"|"never"|"error",
 *            latest?: string, priorAttested?: string, detail?: string}}
 */
export function classifyPackage(name, packument) {
  const latest = packument?.["dist-tags"]?.latest;
  if (!latest) {
    return { name, status: "error", detail: "no `latest` dist-tag on the registry" };
  }
  const versions = packument?.versions ?? {};
  const latestRecord = versions[latest];
  if (!latestRecord) {
    return {
      name,
      status: "error",
      latest,
      detail: `dist-tags.latest points at ${latest}, which is not in the packument`,
    };
  }

  if (hasAttestation(latestRecord)) {
    return { name, status: "ok", latest };
  }

  const times = packument?.time ?? {};
  const latestTime = Date.parse(times[latest] ?? "");
  if (Number.isNaN(latestTime)) {
    return { name, status: "error", latest, detail: `no publish time recorded for ${latest}` };
  }

  // pnpm ignores prereleases when the install target is a stable version.
  const latestIsStable = !isPrerelease(latest);

  let priorAttested = null;
  for (const [version, record] of Object.entries(versions)) {
    if (version === latest) continue;
    if (latestIsStable && isPrerelease(version)) continue;
    if (!hasAttestation(record)) continue;
    const published = Date.parse(times[version] ?? "");
    if (Number.isNaN(published) || published >= latestTime) continue;
    // Report the most recent prior attested version — the one a consumer is
    // most likely to be coming from, and the clearest thing to point at.
    if (priorAttested === null || published > priorAttested.published) {
      priorAttested = { version, published };
    }
  }

  if (priorAttested) {
    return { name, status: "regression", latest, priorAttested: priorAttested.version };
  }
  return { name, status: "never", latest };
}

const FETCH_ATTEMPTS = 3;

/**
 * Fetch one packument. Separated so tests can inject a fake.
 *
 * Retries transient failures (network errors and 5xx/429) before giving up: an
 * `error` verdict fails the job, files a tracking issue and pages IFTTT, and a
 * one-off registry blip on a weekly scheduled leg is exactly the manufactured
 * non-event drift-net.yml's header argues against. A 4xx other than 429 is a
 * real answer from the registry and is not retried.
 */
export async function fetchPackument(
  name,
  { fetchImpl = fetch, sleepImpl = sleep, attempts = FETCH_ATTEMPTS } = {},
) {
  const url = `${REGISTRY}/${name.replace("/", "%2f")}`;
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    let response;
    try {
      response = await fetchImpl(url, { headers: { accept: "application/json" } });
    } catch (error) {
      lastError = new Error(`${name}: ${error.message}`);
      if (attempt < attempts) await sleepImpl(attempt);
      continue;
    }
    if (response.ok) return response.json();
    lastError = new Error(`${name}: registry responded ${response.status} ${response.statusText}`);
    // Attached so callers (e.g. check-provenance-preflight.mjs, which treats a
    // 404 as "never published" rather than a registry failure) can tell a 404
    // apart from every other terminal status without re-parsing the message.
    lastError.status = response.status;
    if (response.status < 500 && response.status !== 429) break;
    if (attempt < attempts) await sleepImpl(attempt);
  }
  throw lastError;
}

function sleep(attempt) {
  return new Promise((resolve) => setTimeout(resolve, attempt * 1000));
}

/**
 * Read the owner policy committed next to this script. Missing file means no
 * acknowledgements. A malformed file throws so the weekly check fails closed.
 */
export function loadProvenancePolicy(path = PROVENANCE_POLICY_PATH) {
  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
  const policy = JSON.parse(text);
  if (policy?.schema !== "zfb-provenance-policy/v1" || !Array.isArray(policy.decisions)) {
    throw new Error(`${path}: invalid provenance policy`);
  }
  return policy;
}

/**
 * Turn a regression into an acknowledgement when the owner's policy covers
 * that package at a version no newer than `latest`. Other verdicts stay.
 */
export function applyProvenancePolicy(results, policy) {
  if (!policy) return results;
  return results.map((result) => {
    if (result.status !== "regression") return result;
    const decision = policy.decisions.find((item) => item.package === result.name);
    if (!decision || typeof decision.unattested_from_version !== "string") return result;
    let cmp;
    try {
      cmp = compareSemver(result.latest, decision.unattested_from_version);
    } catch (error) {
      return { ...result, status: "error", detail: error.message };
    }
    if (cmp < 0) return result;
    return { ...result, status: "acknowledged", decision };
  });
}

/**
 * Check every published package and return the verdicts.
 *
 * @returns {Promise<Array<ReturnType<typeof classifyPackage>>>}
 */
export async function checkAll({ packages = PUBLISHED_PACKAGES, fetchOne = fetchPackument } = {}) {
  return Promise.all(
    packages.map(async (name) => {
      try {
        return classifyPackage(name, await fetchOne(name));
      } catch (error) {
        return { name, status: "error", detail: error.message };
      }
    }),
  );
}

/**
 * Render the verdicts for a human (and for the GHA log/job summary).
 * Returns the exit code: non-zero iff something needs action.
 */
export function report(results, { log = console.log } = {}) {
  const regressions = results.filter((r) => r.status === "regression");
  const acknowledged = results.filter((r) => r.status === "acknowledged");
  const errors = results.filter((r) => r.status === "error");
  const neverViolations = results.filter((r) => r.status === "never");
  const ok = results.filter((r) => r.status === "ok");

  log(`== npm provenance drift check: ${results.length} package(s) ==`);
  for (const r of [...ok].sort((a, b) => a.name.localeCompare(b.name))) {
    log(`  ok         ${r.name}@${r.latest}`);
  }
  for (const r of neverViolations) {
    log(`  NEVER      ${r.name}@${r.latest} (never attested — policy violation)`);
  }
  for (const r of acknowledged) {
    const decision = r.decision;
    log(
      `  ACKNOWLEDGED ${r.name}@${r.latest} (earlier attested: ${r.priorAttested}) by release/provenance-policy.json: unattested_from_version=${decision.unattested_from_version} decided_by=${decision.decided_by} issue=${decision.issue}`,
    );
  }
  for (const r of regressions) {
    log(`  REGRESSION ${r.name}@${r.latest} lost provenance (${r.priorAttested} had it)`);
  }
  for (const r of errors) {
    log(`  ERROR      ${r.name}: ${r.detail}`);
  }

  for (const r of neverViolations) {
    log(
      `::error title=npm provenance absent::${r.name}@${r.latest} has never carried a provenance attestation. Every published package must be attested; this is a policy violation. See issue #2627.`,
    );
  }
  for (const r of regressions) {
    log(
      `::error title=npm provenance trust downgrade::${r.name}@${r.latest} was published WITHOUT provenance, but ${r.priorAttested} carried one. Consumers using pnpm's trustPolicy=no-downgrade cannot install it (ERR_PNPM_TRUST_DOWNGRADE). npm versions are immutable — the only remedy is a follow-up release through the normal tag/release path. See RELEASE_DAY_CHECKLIST.md and issue #2623.`,
    );
  }
  for (const r of errors) {
    log(`::error title=provenance check error::${r.name}: ${r.detail}`);
  }

  if (regressions.length > 0 || neverViolations.length > 0 || errors.length > 0) {
    log(
      `== FAIL: ${regressions.length} regression(s), ${neverViolations.length} never-attested violation(s), ${errors.length} error(s) ==`,
    );
    return 1;
  }
  log(`== PASS: no unapproved provenance drift ==`);
  return 0;
}

// Only run when executed directly, so the test suite can import the pure parts.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let policy;
  try {
    policy = loadProvenancePolicy();
  } catch (error) {
    process.exitCode = report([
      { name: "provenance-policy", status: "error", detail: error.message },
    ]);
  }
  if (process.exitCode === undefined) {
    const results = applyProvenancePolicy(await checkAll(), policy);
    // process.exitCode, not process.exit(): stdout is a pipe under GitHub Actions,
    // and an immediate exit can truncate the ::error:: annotations just written.
    process.exitCode = report(results);
  }
}
