#!/usr/bin/env node
/**
 * retire-next-dist-tag.mjs
 *
 * Keeps the npm `next` dist-tag honest: **`next` exists only while it points at
 * a version strictly newer than `latest`.** Run after a publish; any `next` tag
 * that is no longer ahead of `latest` is removed from the registry.
 *
 * Why this exists
 * ---------------
 * `latest` is maintained by npm automatically; `next` is not. zfb published its
 * whole 0.1.0-next.N line under `next`, graduated to stable at 1.0.0, shipped one
 * last prerelease (1.1.0-next.1, 2026-07-31) and then never touched the tag
 * again — so `next` sat a full major behind while `latest` walked to 2.7.1.
 * A *frozen* dist-tag is worse than a missing one: it still resolves, so
 * `@takazudo/zfb@next` and any tooling that treats `next` as a live channel
 * silently installed year-old code instead of failing loudly.
 *
 * release.yml already had the forward half of this invariant — the ALSO_LATEST
 * dual-tag gate advances `latest` alongside `next` during the prerelease phase
 * (#481). This is the missing reverse half: the graduation cleanup. Without it
 * every future prerelease line re-freezes its own stale `next` the moment it
 * ships a stable.
 *
 * Design constraints
 * ------------------
 *   - **Never fails a release.** A leftover `next` tag is cosmetic next to a
 *     published artifact; the CLI always exits 0 and reports failures as GitHub
 *     `::warning::` annotations. The invariant self-heals on the next release.
 *     The one exception in *severity* (not exit code) is an authentication
 *     failure: under OIDC-only auth an ENEEDAUTH/E401/E403/EOTP means the
 *     trusted publisher is misconfigured (or "Allow npm dist-tag" is off), which
 *     will equally break the NEXT release's dist-tag writes, so it is reported
 *     as an `::error::` annotation plus a job-summary section. It still exits 0:
 *     the packages are already published, and turning a published release red
 *     would invite a re-run that cannot fix registry-side configuration.
 *   - **OIDC only.** Writes go through npm >= 12.2.0's GitHub OIDC exchange;
 *     scripts/npm-oidc-preflight.mjs refuses (no write attempted) when OIDC is
 *     unavailable or any stored npm credential is visible.
 *   - **Read-compare-write.** The dist-tags are re-read immediately before each
 *     `rm` attempt and the decision is re-made on that fresh state, so a
 *     concurrent publish that moved `next` ahead is left alone; after `rm` they
 *     are re-read once more to confirm `next` is gone. npm offers no
 *     compare-and-swap, so a write racing between that last read and the rm
 *     cannot be excluded — the post-write re-read reports it.
 *   - **Semver-aware, not "always remove".** A genuine soak (latest=2.7.2 stable
 *     while next=3.0.0-next.1 is being tested) must survive a stable patch
 *     release, so a `next` that is still ahead of `latest` is left alone.
 *   - **No semver dependency.** The workspace root does not resolve `semver`, and
 *     a release-time script should not reach for one; the comparison below
 *     implements the semver 2.0.0 precedence rules directly.
 */

import { execFile } from "node:child_process";
import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { npmOidcPreflight } from "./npm-oidc-preflight.mjs";

const execFileAsync = promisify(execFile);

/**
 * Every package this repo publishes, in the same order as
 * scripts/advance-latest-dist-tag.sh. The two lists must stay identical — a
 * package that gets `latest` advanced but never gets `next` retired is exactly
 * the drift this script exists to prevent. tests/unit/retire-next-dist-tag.sh
 * asserts the two lists match each other AND the workspace's set of
 * non-private packages, so adding a package to only one place fails the gate.
 */
export const PUBLISHED_PACKAGES = [
  "@takazudo/zfb-darwin-arm64",
  "@takazudo/zfb-darwin-x64",
  "@takazudo/zfb-linux-arm64-gnu",
  "@takazudo/zfb-linux-x64-gnu",
  "@takazudo/zfb-win32-x64-msvc",
  "@takazudo/zfb-slugify",
  "@takazudo/zfb",
  "@takazudo/zfb-runtime",
  "@takazudo/zfb-adapter-cloudflare",
  "create-zfb",
  "@takazudo/zfb-md-wasm",
];

const SEMVER_RE = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

/** Split "1.2.3-next.4+build" into its precedence-relevant parts, or null. */
export function parseSemver(version) {
  const match = SEMVER_RE.exec(String(version ?? "").trim());
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    // Build metadata is deliberately dropped — semver 2.0.0 §10: it is ignored
    // when determining version precedence.
    prerelease: match[4] === undefined ? [] : match[4].split("."),
  };
}

function comparePrereleaseIdentifiers(a, b) {
  const aNumeric = /^\d+$/.test(a);
  const bNumeric = /^\d+$/.test(b);
  // semver 2.0.0 §11: numeric identifiers always have lower precedence than
  // alphanumeric ones, and numeric ones compare numerically (so next.10 > next.9,
  // which a plain string compare gets backwards).
  if (aNumeric && bNumeric) return Math.sign(Number(a) - Number(b));
  if (aNumeric) return -1;
  if (bNumeric) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * Semver 2.0.0 precedence: -1 if a < b, 0 if equal, 1 if a > b.
 * Throws on an unparseable input rather than guessing — a malformed version
 * here would otherwise silently decide whether a tag gets deleted.
 */
export function compareSemver(a, b) {
  const left = parseSemver(a);
  const right = parseSemver(b);
  if (!left) throw new Error(`unparseable version: ${a}`);
  if (!right) throw new Error(`unparseable version: ${b}`);

  for (const field of ["major", "minor", "patch"]) {
    if (left[field] !== right[field]) return Math.sign(left[field] - right[field]);
  }

  // A version WITH a prerelease has lower precedence than one without (§11).
  if (left.prerelease.length === 0 && right.prerelease.length === 0) return 0;
  if (left.prerelease.length === 0) return 1;
  if (right.prerelease.length === 0) return -1;

  const shared = Math.min(left.prerelease.length, right.prerelease.length);
  for (let i = 0; i < shared; i += 1) {
    const cmp = comparePrereleaseIdentifiers(left.prerelease[i], right.prerelease[i]);
    if (cmp !== 0) return cmp;
  }
  // A larger set of prerelease fields wins when all preceding ones are equal.
  return Math.sign(left.prerelease.length - right.prerelease.length);
}

/**
 * The whole policy, as a pure function of one package's dist-tags.
 * Returns { action: "absent" | "keep" | "retire", reason }.
 */
export function decideNextTagAction(distTags) {
  const next = distTags?.next;
  const latest = distTags?.latest;

  if (!next) return { action: "absent", reason: "no 'next' dist-tag" };
  // No stable has ever shipped, so the prerelease line IS the release line.
  if (!latest) return { action: "keep", reason: "no 'latest' yet (prerelease phase)" };

  let cmp;
  try {
    cmp = compareSemver(next, latest);
  } catch (error) {
    // Unparseable on either side: refuse to delete on a guess.
    return { action: "keep", reason: `cannot compare (${error.message})` };
  }

  if (cmp > 0) {
    return { action: "keep", reason: `next=${next} is ahead of latest=${latest} (active soak)` };
  }
  return { action: "retire", reason: `next=${next} is not ahead of latest=${latest}` };
}

/**
 * True for npm failures that a retry cannot possibly fix: authentication and
 * permission errors (EOTP was the first one observed — the backoff loop below
 * once burned 75s per package producing identical failures). Under OIDC an
 * ENEEDAUTH means the token exchange did not yield a credential. Only network
 * blips and registry 5xx deserve a retry.
 */
export function isTerminalNpmError(message) {
  return /\bE(OTP|NEEDAUTH|401|403)\b/.test(String(message ?? ""));
}

async function runNpm(args) {
  const { stdout } = await execFileAsync("npm", args, { encoding: "utf8" });
  return stdout;
}

async function readDistTags(pkg, npm) {
  const stdout = await npm(["view", pkg, "dist-tags", "--json", "--prefer-online"]);
  const trimmed = stdout.trim();
  return trimmed === "" ? {} : JSON.parse(trimmed);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function writeAuthFailureSummary(summaryPath, results) {
  if (!summaryPath) return;
  const failedPkgs = results.filter((r) => r.terminal).map((r) => `\`${r.pkg}\``);
  try {
    appendFileSync(
      summaryPath,
      [
        "### :x: npm dist-tag auth failure ('next' retirement)",
        "",
        failedPkgs.length > 0
          ? `npm refused \`dist-tag rm\` for ${failedPkgs.join(", ")} with an authentication error, and the sweep stopped.`
          : "The npm OIDC preflight refused (no GitHub OIDC, a stored npm credential, or an npm older than 12.2.0); no dist-tag was touched.",
        "The packages are published; this step still exits 0 by policy, but the next release's dist-tag",
        "writes will fail the same way until the npm trusted publisher is fixed (repository, workflow",
        "`release.yml`, and the **Allow npm dist-tag** option).",
        "",
      ].join("\n"),
    );
  } catch {
    // A summary write must never fail the step.
  }
}

/**
 * Retire the `next` dist-tag wherever it is no longer ahead of `latest`.
 * Callers that write (not dry-run) must have passed the OIDC preflight; the CLI
 * below enforces that before calling this.
 *
 * @param {object} [options]
 * @param {boolean} [options.dryRun]  Decide and report, but never mutate the registry.
 * @param {string[]} [options.packages]
 * @param {(args: string[]) => Promise<string>} [options.npm]  Injected for tests.
 * @param {(ms: number) => Promise<void>} [options.wait]       Injected for tests.
 * @param {(line: string) => void} [options.log]
 * @returns {Promise<{results: Array<object>, failed: number}>} Never rejects.
 */
export async function retireNextDistTag(options = {}) {
  const {
    dryRun = false,
    packages = PUBLISHED_PACKAGES,
    npm = runNpm,
    wait = sleep,
    log = console.log,
    maxAttempts = 5,
    summaryPath = process.env.GITHUB_STEP_SUMMARY,
  } = options;

  const results = [];
  let failed = 0;
  let authFailed = false;

  for (const pkg of packages) {
    let distTags;
    try {
      distTags = await readDistTags(pkg, npm);
    } catch (error) {
      failed += 1;
      log(`::warning::${pkg}: could not read dist-tags (${error.message}) — 'next' left as-is.`);
      results.push({ pkg, action: "error", reason: error.message });
      continue;
    }

    const decision = decideNextTagAction(distTags);
    if (decision.action !== "retire") {
      log(`  ${pkg}: ${decision.action} — ${decision.reason}`);
      results.push({ pkg, ...decision });
      continue;
    }

    if (dryRun) {
      log(`  ${pkg}: would remove 'next' — ${decision.reason}`);
      results.push({ pkg, action: "retire", dryRun: true, reason: decision.reason });
      continue;
    }

    const rmArgs = ["dist-tag", "rm", pkg, "next"];

    let outcome = null; // "retired" | "changed" once settled
    let terminal = false;
    let lastError = "";
    let delay = 5000;
    let currentReason = decision.reason;
    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      try {
        // Re-read immediately before the write and re-decide on fresh state:
        // a concurrent publish may have moved `next` ahead (or removed it).
        const fresh = decideNextTagAction(await readDistTags(pkg, npm));
        if (fresh.action !== "retire") {
          outcome = "changed";
          currentReason = fresh.reason;
          break;
        }
        currentReason = fresh.reason;
        await npm(rmArgs);
        const after = await readDistTags(pkg, npm);
        if (!after?.next) {
          log(
            `  ${pkg}: removed 'next' (${currentReason}) — OK, confirmed by re-read (attempt ${attempt})`,
          );
          outcome = "retired";
          break;
        }
        lastError = `re-read after rm still shows next=${after.next}`;
      } catch (error) {
        lastError = error.message;
        if (isTerminalNpmError(lastError)) {
          terminal = true;
          break;
        }
      }
      if (attempt < maxAttempts) {
        log(
          `  ${pkg}: dist-tag rm not confirmed (attempt ${attempt}/${maxAttempts}: ${lastError}) — retrying in ${delay / 1000}s...`,
        );
        await wait(delay);
        delay *= 2;
      }
    }

    if (outcome === "changed") {
      log(
        `  ${pkg}: left 'next' in place — dist-tags changed before the write (${currentReason}).`,
      );
      results.push({ pkg, action: "changed", reason: currentReason });
    } else if (outcome === "retired") {
      results.push({ pkg, action: "retired", reason: currentReason });
    } else if (terminal) {
      failed += 1;
      authFailed = true;
      log(
        `::error title=npm dist-tag auth failure::${pkg}: 'npm dist-tag rm' was refused with an authentication error (${lastError}). The release workflow authenticates only through GitHub OIDC trusted publishing: check the package's trusted publisher (repository, workflow release.yml) and that its "Allow npm dist-tag" option is enabled. Not retried.`,
      );
      results.push({ pkg, action: "error", terminal: true, reason: lastError });
      // One terminal auth failure means every remaining package will fail the
      // same way. Stop rather than replaying the same error ten times.
      log(
        `::error title=npm dist-tag auth failure::Aborting the 'next' sweep after an auth failure; remaining packages were not attempted.`,
      );
      break;
    } else {
      failed += 1;
      log(
        `::warning::${pkg}: could not remove the stale 'next' dist-tag after ${maxAttempts} attempts (${lastError}). Manual remediation (maintainer's own interactive npm session): npm dist-tag rm ${pkg} next`,
      );
      results.push({ pkg, action: "error", terminal: false, reason: lastError });
    }
  }

  if (authFailed) writeAuthFailureSummary(summaryPath, results);
  return { results, failed, authFailed };
}

/** Annotation lines for a preflight refusal (exported for tests). */
export function preflightRefusalLines(problems) {
  return [
    ...problems.map((p) => `::error title=npm dist-tag auth failure::${p}`),
    "::error title=npm dist-tag auth failure::npm OIDC preflight refused; no 'next' dist-tag was touched.",
  ];
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const dryRun = process.argv.includes("--dry-run");
  console.log(
    `Retiring stale 'next' dist-tags${dryRun ? " (DRY RUN — no registry writes)" : ""}...`,
  );
  if (!dryRun) {
    const problems = npmOidcPreflight();
    if (problems.length > 0) {
      for (const line of preflightRefusalLines(problems)) console.log(line);
      writeAuthFailureSummary(process.env.GITHUB_STEP_SUMMARY, []);
      // Always 0 (see the header): the refusal is surfaced as ::error::.
      process.exit(0);
    }
  }
  const { results, failed } = await retireNextDistTag({ dryRun });
  const retired = results.filter((r) => r.action === "retired" || r.dryRun).length;
  console.log(
    `'next' dist-tag sweep complete: ${retired} retired, ${results.length - retired - failed} left in place, ${failed} failed.`,
  );
  // Always 0: a leftover dist-tag must never redden an otherwise good release.
  // Failures are surfaced as ::warning:: (auth: ::error::) annotations above.
  process.exit(0);
}
