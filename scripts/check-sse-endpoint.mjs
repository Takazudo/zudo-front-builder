#!/usr/bin/env node
//
// check-sse-endpoint.mjs — keep the /__zfb/reload endpoint literal confined to
// the code that owns it and the one bounded embed-mode probe that must name it.
//
// This is deliberately a source guard rather than a Rust/runtime test. A
// hand-written SSE subscriber in a test can look plausible while using
// reqwest's total request timeout incorrectly: `.timeout()` bounds the whole
// streaming response, and the server keeps the stream alive for 15s. Tests
// should use `zfb_test_utils::open_sse()` instead of spelling the endpoint.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..");
const DEFAULT_CRATES_ROOT = join(REPO_ROOT, "crates");

export const ENDPOINT_LITERAL = "__zfb/reload";

// These are the only source directories allowed to define/use the endpoint
// literal. Keep the `crates/` prefix in the path representation so diagnostics
// match the paths developers see in the repository and in GitHub review.
export const ALLOWED_SOURCE_PREFIXES = ["crates/zfb-server/src/", "crates/zfb-test-utils/src/"];

// Every non-subscriber exception needs a reason. This is intentionally a
// small data allowlist, not a broad directory exemption. The embed smoke test
// names the route in a bounded HTTP probe to assert that Embed mode does NOT
// mount the endpoint; it does not open or consume an SSE stream.
export const ENDPOINT_ALLOWLIST = new Map([
  [
    "crates/zfb-server/tests/embed_lifecycle_smoke.rs",
    "bounded Embed-mode 404 probe verifies that the SSE endpoint is not mounted; it does not subscribe to the stream",
  ],
]);

/** Convert a native path into the repository's slash-separated path form. */
function toRepoPath(file, cratesRoot) {
  return `crates/${relative(cratesRoot, file).split(sep).join("/")}`;
}

/** Return true only for a complete allowed source-directory prefix. */
function isAllowedSourcePath(repoPath) {
  return ALLOWED_SOURCE_PREFIXES.some((prefix) => repoPath.startsWith(prefix));
}

/** Recursively list every Rust source file below `root` in stable order. */
function listRustFiles(root) {
  const files = [];

  function walk(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      const file = join(directory, entry.name);
      if (entry.isDirectory()) {
        walk(file);
      } else if (entry.name.endsWith(".rs") && (entry.isFile() || entry.isSymbolicLink())) {
        files.push(file);
      }
    }
  }

  walk(root);
  return files;
}

/**
 * Check the endpoint-literal invariant.
 *
 * `cratesRoot` is injectable so the offline shell test can run this exact
 * scanner against temporary fixture trees without touching the repository.
 * The allowlist remains repository-relative (`crates/...`) in those fixtures.
 *
 * @returns {{ files: number, uses: number, findings: string[], accepted: string[] }}
 */
export function checkSseEndpoint({ cratesRoot = DEFAULT_CRATES_ROOT } = {}) {
  const root = resolve(cratesRoot);
  const pathBase = dirname(root);
  const findings = [];
  const accepted = [];
  const filesByRepoPath = new Map();

  if (!existsSync(root)) {
    return {
      files: 0,
      uses: 0,
      findings: [`expected crates directory does not exist: ${root}`],
      accepted,
    };
  }

  let files;
  try {
    files = listRustFiles(root);
  } catch (error) {
    return {
      files: 0,
      uses: 0,
      findings: [`could not scan Rust files under ${root}: ${error.message}`],
      accepted,
    };
  }

  if (files.length === 0) {
    findings.push(`found no .rs files under ${root}; endpoint scan is not active`);
  }

  let uses = 0;
  for (const file of files) {
    const repoPath = toRepoPath(file, root);
    let source;
    try {
      source = readFileSync(file, "utf8");
    } catch (error) {
      findings.push(`could not read ${repoPath}: ${error.message}`);
      continue;
    }
    filesByRepoPath.set(repoPath, source);

    if (!source.includes(ENDPOINT_LITERAL)) continue;
    uses += 1;

    if (isAllowedSourcePath(repoPath)) {
      accepted.push(`${repoPath} (allowed source home)`);
    } else if (ENDPOINT_ALLOWLIST.has(repoPath)) {
      accepted.push(`${repoPath} (allowlist: ${ENDPOINT_ALLOWLIST.get(repoPath)})`);
    } else {
      findings.push(
        `forbidden ${JSON.stringify(ENDPOINT_LITERAL)} literal in ${repoPath}. ` +
          "Reqwest's `.timeout()` bounds the whole streaming response, while the server keep-alive is 15s; " +
          "use `zfb_test_utils::open_sse()` for SSE subscriptions.",
      );
    }
  }

  // A path in the allowlist must continue to be a real violation: if the file
  // is deleted, moved, fixed, or moved into an allowed source home, the entry
  // becomes stale and fails the guard instead of silently becoming permanent
  // cover for future endpoint literals.
  for (const [repoPath, reason] of ENDPOINT_ALLOWLIST) {
    if (typeof reason !== "string" || reason.trim() === "") {
      findings.push(`allowlist entry ${repoPath} has no stated reason`);
      continue;
    }

    if (!repoPath.startsWith("crates/") || !repoPath.endsWith(".rs")) {
      findings.push(`allowlist entry ${repoPath} is not a repository-relative .rs path`);
      continue;
    }

    const expectedFile = resolve(pathBase, repoPath);
    const expectedRoot = `${root}${sep}`;
    if (expectedFile !== root && !expectedFile.startsWith(expectedRoot)) {
      findings.push(`allowlist entry ${repoPath} points outside ${root}`);
      continue;
    }

    const source = filesByRepoPath.get(repoPath);
    if (source === undefined) {
      // Distinguish a missing file from a path that exists but was not scanned
      // (for example, a directory or an unreadable entry), while keeping both
      // cases fail-closed.
      const detail = existsSync(expectedFile)
        ? "is not a readable Rust file under the scan"
        : "does not exist";
      findings.push(`stale allowlist entry ${repoPath}: ${detail}`);
      continue;
    }

    if (!source.includes(ENDPOINT_LITERAL) || isAllowedSourcePath(repoPath)) {
      findings.push(
        `stale allowlist entry ${repoPath}: it no longer matches a forbidden ${JSON.stringify(ENDPOINT_LITERAL)} use`,
      );
    }
  }

  return { files: files.length, uses, findings, accepted };
}

/** Print a concise, CI-friendly verdict and return the process exit code. */
export function report(result, { log = console.log, error = console.error } = {}) {
  for (const item of result.accepted.sort()) log(`OK: ${item}`);
  for (const finding of result.findings) error(`FAIL: ${finding}`);

  if (result.findings.length > 0) {
    error(
      `SSE endpoint literal guard failed: scanned ${result.files} .rs file(s), ` +
        `found ${result.uses} file(s) containing ${JSON.stringify(ENDPOINT_LITERAL)}.`,
    );
    return 1;
  }

  log(
    `PASS: ${JSON.stringify(ENDPOINT_LITERAL)} is confined to its two source homes and the reasoned allowlist ` +
      `(${result.files} .rs file(s) scanned, ${result.uses} file(s) containing it).`,
  );
  return 0;
}

function main() {
  const result = checkSseEndpoint({
    cratesRoot: process.env.CHECK_SSE_ENDPOINT_CRATES_ROOT ?? DEFAULT_CRATES_ROOT,
  });
  process.exitCode = report(result);
}

// Only run when executed directly, so tests can import the scanner/report
// without starting a repository scan as an import side effect.
const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(resolve(argument)).href) main();
