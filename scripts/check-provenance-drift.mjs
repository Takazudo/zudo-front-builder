#!/usr/bin/env node
//
// check-provenance-drift.mjs — catch a published package that LOST its npm
// provenance attestation, without going red over one that never had it.
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
// THE CENTRAL DESIGN DECISION: regression fails, standing absence warns
// ---------------------------------------------------------------------------
// `@takazudo/zfb-darwin-x64` has never carried an attestation on any recent
// release (issue #2625) — it is built locally on the fast-Mac path, which has no
// GHA OIDC context. Failing on "no attestation" would make this job red EVERY
// week over a known, tracked, undecided situation, which is precisely the
// failure mode drift-net.yml's own header argues against for the `next` channel:
// a scheduled leg that manufactures a tracking issue and an alert out of a
// non-event trains everyone to ignore it.
//
// So this mirrors pnpm's actual semantics instead:
//   REGRESSION  — latest has no attestation, but an EARLIER-PUBLISHED version
//                 did. This is a real trust downgrade for consumers. FAIL.
//   NEVER       — latest has no attestation and no earlier version did either.
//                 No downgrade can fire (pnpm needs an earlier attested version
//                 to compare against), so this is a weaker standing gap. WARN.
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

import { PUBLISHED_PACKAGES } from "./retire-next-dist-tag.mjs";

const REGISTRY = "https://registry.npmjs.org";

/** True when this packument version record carries a provenance attestation. */
function hasAttestation(versionRecord) {
  return Boolean(versionRecord?.dist?.attestations);
}

/** A version is a prerelease when its semver carries a `-` suffix. */
function isPrerelease(version) {
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

/** Fetch one packument. Separated so tests can inject a fake. */
async function fetchPackument(name) {
  const url = `${REGISTRY}/${name.replace("/", "%2f")}`;
  const response = await fetch(url, {
    headers: { accept: "application/json" },
  });
  if (!response.ok) {
    throw new Error(`${name}: registry responded ${response.status} ${response.statusText}`);
  }
  return response.json();
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
  const errors = results.filter((r) => r.status === "error");
  const never = results.filter((r) => r.status === "never");
  const ok = results.filter((r) => r.status === "ok");

  log(`== npm provenance drift check: ${results.length} package(s) ==`);
  for (const r of [...ok].sort((a, b) => a.name.localeCompare(b.name))) {
    log(`  ok         ${r.name}@${r.latest}`);
  }
  for (const r of never) {
    log(`  no-attest  ${r.name}@${r.latest} (never attested — standing gap, not a downgrade)`);
  }
  for (const r of regressions) {
    log(`  REGRESSION ${r.name}@${r.latest} lost provenance (${r.priorAttested} had it)`);
  }
  for (const r of errors) {
    log(`  ERROR      ${r.name}: ${r.detail}`);
  }

  if (never.length > 0) {
    log(
      `::warning title=npm provenance absent::${never.length} package(s) have never carried a provenance attestation: ${never
        .map((r) => r.name)
        .join(
          ", ",
        )}. Not a trust downgrade (pnpm needs an earlier attested version to compare against), so this does not fail the check — see issue #2625.`,
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

  if (regressions.length > 0 || errors.length > 0) {
    log(`== FAIL: ${regressions.length} regression(s), ${errors.length} error(s) ==`);
    return 1;
  }
  log(`== PASS: no package lost an attestation it previously had ==`);
  return 0;
}

// Only run when executed directly, so the test suite can import the pure parts.
if (import.meta.url === `file://${process.argv[1]}`) {
  const results = await checkAll();
  process.exit(report(results));
}
