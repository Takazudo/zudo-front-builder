#!/usr/bin/env node
//
// check-provenance-preflight.mjs — pre-publish guard for the same npm trust
// downgrade check-provenance-drift.mjs catches after the fact (issue #3134).
//
// WHY THIS EXISTS
// ---------------------------------------------------------------------------
// v2.20.3 published `@takazudo/zfb-darwin-x64` via `--fast-mac`, which cannot
// attach provenance, after every earlier version back to 2.13.0 carried one.
// pnpm's `trustPolicy: no-downgrade` compares by publish date: any consumer
// (including a Linux one, since pnpm resolves all optionalDependencies) then
// fails with ERR_PNPM_TRUST_DOWNGRADE. Before this script, the only signals
// were a non-fatal `::warning` in scripts/publish-npm-packages.sh and the
// weekly after-the-fact check-provenance-drift.mjs run. This script runs
// BEFORE publishing, so a release that would regress trust can be blocked (or
// explicitly acknowledged) instead of discovered by a consumer.
//
// This sub-task (#3136) delivers the script and its tests only. Wiring it
// into release.yml / publish-npm-packages.sh so it actually blocks a release
// is a sibling sub-issue (#3140, "A2").
//
// DESIGN
// ---------------------------------------------------------------------------
// `wouldDowngrade` mirrors check-provenance-drift.mjs's `classifyPackage`
// prerelease rule (prereleases are ignored once the target is a stable
// version, matching pnpm >= 10.24) but compares against an explicit TARGET
// version instead of the registry's current `dist-tags.latest` — the version
// about to be published has not been published yet, so it cannot appear in
// the packument at all.
//
// A package that already carries the target version is skipped rather than
// checked: scripts/publish-npm-packages.sh / release.yml skip versions that
// are already published (release.yml:~1052), so a retried or partially-failed
// publish must not false-fail here on its own prior success.
//
// Registry failures fail CLOSED (exit 2): a release can be retried after a
// registry blip, but a shipped downgrade is permanent (npm versions are
// immutable). A 404 (package never published) is the one non-error absence —
// it cannot possibly be a downgrade.

import { pathToFileURL } from "node:url";

import { fetchPackument, hasAttestation, isPrerelease } from "./check-provenance-drift.mjs";
import { PUBLISHED_PACKAGES } from "./retire-next-dist-tag.mjs";

/** The one darwin-x64 package `--fast-mac` publishes (the 2.20.3 incident). */
const MAC_LOCAL_PACKAGES = ["@takazudo/zfb-darwin-x64"];

const MODES = {
  "mac-local": MAC_LOCAL_PACKAGES,
  "recovery-no-provenance": PUBLISHED_PACKAGES,
};

/**
 * Pure: would publishing `version` for this package be a trust downgrade?
 *
 * True when any version in the packument OTHER than `version` carries a
 * provenance attestation (prereleases excluded when `version` itself is
 * stable, counted when `version` is itself a prerelease — the same rule
 * check-provenance-drift.mjs's classifyPackage applies against `latest`).
 *
 * @returns {string|null} the earliest such attested version (by publish
 *   date; falls back to first-seen when publish times are missing), or null
 *   when publishing `version` would not regress trust.
 */
export function wouldDowngrade(packument, { version }) {
  const versions = packument?.versions ?? {};
  const times = packument?.time ?? {};
  const targetIsStable = !isPrerelease(version);

  let earliest = null;
  let earliestPublished = Number.POSITIVE_INFINITY;
  for (const [candidate, record] of Object.entries(versions)) {
    if (candidate === version) continue;
    if (targetIsStable && isPrerelease(candidate)) continue;
    if (!hasAttestation(record)) continue;

    const published = Date.parse(times[candidate] ?? "");
    const rank = Number.isNaN(published) ? Number.POSITIVE_INFINITY : published;
    if (earliest === null || rank < earliestPublished) {
      earliest = candidate;
      earliestPublished = rank;
    }
  }
  return earliest;
}

/**
 * Check one package against the target version.
 *
 * @returns {Promise<{name: string, status: "ok"|"skipped"|"downgrade",
 *   version?: string, earlierAttested?: string, detail?: string}>}
 * @throws when the packument cannot be fetched for a reason other than 404
 *   (the caller must fail closed — see the module header).
 */
export async function checkPackage(name, { version, fetchOne = fetchPackument } = {}) {
  let packument;
  try {
    packument = await fetchOne(name);
  } catch (error) {
    if (error?.status === 404) {
      return { name, status: "ok", detail: "package has never been published" };
    }
    throw error;
  }

  const versions = packument?.versions ?? {};
  if (Object.prototype.hasOwnProperty.call(versions, version)) {
    return { name, status: "skipped", detail: `${version} is already published` };
  }

  const earlierAttested = wouldDowngrade(packument, { version });
  if (earlierAttested) {
    return { name, status: "downgrade", version, earlierAttested };
  }
  return { name, status: "ok" };
}

/**
 * Check every package for a mode. Never rejects: a per-package fetch failure
 * (other than 404) is captured as an `error` verdict so the caller can fail
 * closed after seeing every package's outcome, rather than stopping at the
 * first registry blip and leaving the rest unchecked.
 *
 * @returns {Promise<Array<Awaited<ReturnType<typeof checkPackage>> |
 *   {name: string, status: "error", detail: string}>>}
 */
export async function checkMode(mode, { version, packages = MODES[mode], fetchOne } = {}) {
  if (!packages) {
    throw new Error(`unknown --mode: ${mode} (expected one of: ${Object.keys(MODES).join(", ")})`);
  }
  return Promise.all(
    packages.map(async (name) => {
      try {
        return await checkPackage(name, { version, fetchOne });
      } catch (error) {
        return { name, status: "error", detail: error.message };
      }
    }),
  );
}

/**
 * Render the verdicts and return the process exit code:
 *   0 — no downgrade; 1 — at least one downgrade, no errors; 2 — a registry
 *   error means the picture is incomplete, so fail closed regardless of what
 *   the other packages showed.
 */
export function report(results, { log = console.log } = {}) {
  const downgrades = results.filter((r) => r.status === "downgrade");
  const errors = results.filter((r) => r.status === "error");

  for (const r of results) {
    if (r.status === "ok") log(`  ok       ${r.name}`);
    else if (r.status === "skipped") log(`  skipped  ${r.name} (${r.detail})`);
    else if (r.status === "downgrade") {
      log(`DOWNGRADE ${r.name}@${r.version} (earlier attested: ${r.earlierAttested})`);
    } else if (r.status === "error") log(`  ERROR    ${r.name}: ${r.detail}`);
  }

  if (errors.length > 0) {
    log(
      `== FAIL (registry error): ${errors.length} package(s) could not be checked, failing closed ==`,
    );
    return 2;
  }
  if (downgrades.length > 0) {
    log(
      `== FAIL: publishing would be a provenance trust downgrade for ${downgrades.length} package(s) ==`,
    );
    return 1;
  }
  log(`== PASS: no provenance trust downgrade ==`);
  return 0;
}

/** Parse `--version <v> --mode <m>`. Returns null (not throws) on bad args. */
export function parseArgs(argv) {
  let version;
  let mode;
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--version") version = argv[(i += 1)];
    else if (argv[i] === "--mode") mode = argv[(i += 1)];
  }
  if (!version || !mode) return null;
  return { version, mode };
}

async function main(argv) {
  const args = parseArgs(argv);
  if (!args) {
    console.error(
      "usage: node scripts/check-provenance-preflight.mjs --version <v> --mode mac-local|recovery-no-provenance",
    );
    return 2;
  }
  if (!MODES[args.mode]) {
    console.error(
      `unknown --mode: ${args.mode} (expected one of: ${Object.keys(MODES).join(", ")})`,
    );
    return 2;
  }

  const results = await checkMode(args.mode, { version: args.version });
  return report(results);
}

// Only run when executed directly, so the test suite can import the pure parts.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // process.exitCode, not process.exit(): stdout is a pipe under GitHub
  // Actions, and an immediate exit can truncate output written just before it.
  process.exitCode = await main(process.argv.slice(2));
}
