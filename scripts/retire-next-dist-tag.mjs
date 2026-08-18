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
 *   - **Semver-aware, not "always remove".** A genuine soak (latest=2.7.2 stable
 *     while next=3.0.0-next.1 is being tested) must survive a stable patch
 *     release, so a `next` that is still ahead of `latest` is left alone.
 *   - **No semver dependency.** The workspace root does not resolve `semver`, and
 *     a release-time script should not reach for one; the comparison below
 *     implements the semver 2.0.0 precedence rules directly.
 */

import { execFile } from "node:child_process";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

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
 * True for npm failures that a retry cannot possibly fix. EOTP (the account
 * requires a one-time password per write) is the motivating case: an interactive
 * `npm login` session authenticates but still demands an OTP for each dist-tag
 * write, so the backoff loop below burned 75s per package to produce ten
 * identical failures. Auth/permission errors are equally terminal — only
 * network blips and registry 5xx deserve a retry.
 */
export function isTerminalNpmError(message) {
  return /\bE(OTP|NEEDAUTH|401|403)\b/.test(String(message ?? ""));
}

async function runNpm(args) {
  const { stdout } = await execFileAsync("npm", args, { encoding: "utf8" });
  return stdout;
}

async function readDistTags(pkg, npm) {
  const stdout = await npm(["view", pkg, "dist-tags", "--json"]);
  const trimmed = stdout.trim();
  return trimmed === "" ? {} : JSON.parse(trimmed);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Retire the `next` dist-tag wherever it is no longer ahead of `latest`.
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
    otp,
  } = options;

  const results = [];
  let failed = 0;

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
    if (otp) rmArgs.push("--otp", otp);

    let removed = false;
    let terminal = false;
    let lastError = "";
    let delay = 5000;
    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      try {
        await npm(rmArgs);
        log(`  ${pkg}: removed 'next' (${decision.reason}) — OK (attempt ${attempt})`);
        removed = true;
        break;
      } catch (error) {
        lastError = error.message;
        if (isTerminalNpmError(lastError)) {
          terminal = true;
          break;
        }
        if (attempt < maxAttempts) {
          log(
            `  ${pkg}: dist-tag rm failed (attempt ${attempt}/${maxAttempts}) — retrying in ${delay / 1000}s...`,
          );
          await wait(delay);
          delay *= 2;
        }
      }
    }

    if (removed) {
      results.push({ pkg, action: "retired", reason: decision.reason });
    } else {
      failed += 1;
      const attempts = terminal ? "an auth error (not retried)" : `${maxAttempts} attempts`;
      log(
        `::warning::${pkg}: could not remove the stale 'next' dist-tag after ${attempts} (${lastError}). Manual remediation: npm dist-tag rm ${pkg} next`,
      );
      results.push({ pkg, action: "error", terminal, reason: lastError });

      // One terminal auth failure means every remaining package will fail the
      // same way. Stop rather than replaying the same error ten times.
      if (terminal) {
        log(
          `::warning::Aborting the sweep: this npm account needs re-auth or a per-write OTP. Re-run with --otp=<code>, or use an Automation token (which bypasses 2FA) as release.yml does.`,
        );
        break;
      }
    }
  }

  return { results, failed };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const dryRun = process.argv.includes("--dry-run");
  const otpArg = process.argv.find((a) => a.startsWith("--otp="));
  const otp = otpArg ? otpArg.slice("--otp=".length) : undefined;
  console.log(
    `Retiring stale 'next' dist-tags${dryRun ? " (DRY RUN — no registry writes)" : ""}...`,
  );
  const { results, failed } = await retireNextDistTag({ dryRun, otp });
  const retired = results.filter((r) => r.action === "retired" || r.dryRun).length;
  console.log(
    `'next' dist-tag sweep complete: ${retired} retired, ${results.length - retired - failed} left in place, ${failed} failed.`,
  );
  // Always 0: a leftover dist-tag must never redden an otherwise good release.
  // Failures are surfaced as ::warning:: annotations above and retried next release.
  process.exit(0);
}
