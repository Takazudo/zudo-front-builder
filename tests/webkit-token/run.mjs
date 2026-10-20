/**
 * WebKit run of the browser specs that exercise the island build token
 * (`data-zfb-build`) — #4231 item 2, `pnpm test:webkit-token`.
 *
 * LOCAL-HEAVY (T4) — Mac. Not wired into CI or `pnpm test`. It reuses the
 * existing Chromium specs unchanged and only swaps the Playwright project
 * (`ZFB_BROWSER_ENGINE`, default `webkit` here), so a WebKit-only difference
 * shows up as a real failure instead of a re-written assertion.
 *
 * Targets (exact selected counts are part of the contract):
 *   sdk-persist            1 test   router-chromium: token attribute is emitted, an unchanged
 *                                   root keeps its DOM node across a router swap
 *   r-a08-mismatch         1 test   zudo-react-browser: a mismatched `data-zfb-build` raises
 *                                   ZR_IDENTITY, preserves the edited DOM and leaves a healthy
 *                                   neighbouring root active
 *   registry (opt-in)      4 tests  scanner-boundary-acceptance: missing/mismatched SSR token
 *                                   rejection against a product build. Needs the artifacts the
 *                                   Rust scanner_boundary_acceptance test writes to
 *                                   target/scanner-boundary-acceptance/, so it only runs with
 *                                   ZFB_WEBKIT_TOKEN_REGISTRY=1.
 *
 * A target passes only when the selected count equals the contract, every selected
 * test ran in the requested engine, and all of them passed with no skip, no
 * failure and no retry-recovered flake. Zero selected tests is a failure, never a pass.
 *
 * Run from the repo root:
 *   pnpm test:webkit-token
 * Skip the builds when they are already current: ZFB_WEBKIT_TOKEN_SKIP_BUILD=1.
 */
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const ENGINE = process.env.ZFB_BROWSER_ENGINE ?? "webkit";
const SKIP_BUILD = process.env.ZFB_WEBKIT_TOKEN_SKIP_BUILD === "1";
const WITH_REGISTRY = process.env.ZFB_WEBKIT_TOKEN_REGISTRY === "1";
const REPO_ROOT = process.cwd();

const targets = [
  {
    name: "sdk-persist",
    config: "tests/router-chromium/playwright.config.mjs",
    filter: "sdk-persist",
    expected: 1,
    builds: [
      ["pnpm", ["-C", "packages/zfb-slugify", "build"]],
      ["pnpm", ["-C", "packages/zfb-runtime", "build"]],
      ["pnpm", ["-C", "packages/zfb", "build"]],
      ["node", ["tests/router-chromium/build-real-islands-fixture.mjs"]],
    ],
  },
  {
    name: "r-a08-mismatch",
    config: "tests/zudo-react-browser/playwright.config.mjs",
    filter: "r-a08-mismatch",
    expected: 1,
    builds: [["node", ["tests/zudo-react-browser/build-fixture.mjs"]]],
  },
];
if (WITH_REGISTRY) {
  targets.push({
    name: "registry",
    config: "tests/scanner-boundary-acceptance/playwright.config.mjs",
    filter: "registry",
    expected: 4,
    builds: [],
    requires: join(REPO_ROOT, "target", "scanner-boundary-acceptance", "acceptance.json"),
  });
}

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    ...options,
    env: { ...process.env, ZFB_BROWSER_ENGINE: ENGINE, ...options.env },
  });
}

function collectTests(suites, out = []) {
  for (const suite of suites ?? []) {
    for (const spec of suite.specs ?? []) {
      for (const test of spec.tests ?? []) out.push({ title: spec.title, test });
    }
    collectTests(suite.suites, out);
  }
  return out;
}

function playwright(target, extra, env) {
  return run(
    "pnpm",
    [
      "exec",
      "playwright",
      "test",
      "--config",
      target.config,
      "--retries=0",
      ...extra,
      target.filter,
    ],
    { env },
  );
}

function parseJson(text, what) {
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`${what}: output is not JSON (${error.message})`);
  }
}

const results = [];
for (const target of targets) {
  const row = {
    name: target.name,
    selected: 0,
    passed: 0,
    failed: 0,
    skipped: 0,
    flaky: 0,
    ok: false,
    note: "",
  };
  results.push(row);
  try {
    if (target.requires && !existsSync(target.requires)) {
      throw new Error(
        `missing ${target.requires}; run the Rust scanner_boundary_acceptance test first`,
      );
    }
    if (!SKIP_BUILD) {
      for (const [command, args] of target.builds) {
        const build = run(command, args, { stdio: "inherit", encoding: undefined });
        if (build.status !== 0)
          throw new Error(`build step failed: ${command} ${args.join(" ")} (exit ${build.status})`);
      }
    }

    const listing = playwright(target, ["--list", "--reporter=json"]);
    if (listing.status !== 0)
      throw new Error(
        `--list failed (exit ${listing.status}): ${listing.stderr.trim().slice(0, 400)}`,
      );
    const listed = collectTests(parseJson(listing.stdout, "--list").suites);
    row.selected = listed.length;
    if (listed.length !== target.expected) {
      throw new Error(`selected ${listed.length} tests, contract says ${target.expected}`);
    }
    const wrongEngine = listed.filter(({ test }) => test.projectName !== ENGINE);
    if (wrongEngine.length > 0)
      throw new Error(`${wrongEngine.length} selected tests are not in project ${ENGINE}`);

    const outDir = mkdtempSync(join(tmpdir(), "zfb-webkit-token-"));
    const jsonPath = join(outDir, `${target.name}.json`);
    const ran = playwright(target, ["--reporter=json,list"], {
      PLAYWRIGHT_JSON_OUTPUT_NAME: jsonPath,
    });
    process.stdout.write(ran.stdout);
    process.stderr.write(ran.stderr);
    if (!existsSync(jsonPath)) throw new Error(`no JSON report written (exit ${ran.status})`);
    const report = parseJson(readFileSync(jsonPath, "utf8"), "run report");
    const tests = collectTests(report.suites);
    for (const { test } of tests) {
      const status = test.status; // expected | unexpected | flaky | skipped
      if (status === "expected") row.passed += 1;
      else if (status === "skipped") row.skipped += 1;
      else if (status === "flaky") row.flaky += 1;
      else row.failed += 1;
    }
    row.ok =
      ran.status === 0 &&
      tests.length === target.expected &&
      row.passed === target.expected &&
      row.failed === 0 &&
      row.skipped === 0 &&
      row.flaky === 0;
    if (!row.ok) row.note = `exit ${ran.status}`;
  } catch (error) {
    row.note = error.message;
  }
}

console.log(`\nwebkit-token summary (engine=${ENGINE})`);
for (const row of results) {
  console.log(
    `  ${row.ok ? "PASS" : "FAIL"} ${row.name}: selected ${row.selected}, passed ${row.passed}, failed ${row.failed}, skipped ${row.skipped}, flaky ${row.flaky}${row.note ? ` — ${row.note}` : ""}`,
  );
}
if (!WITH_REGISTRY)
  console.log("  (registry target not selected: set ZFB_WEBKIT_TOKEN_REGISTRY=1)");
process.exit(results.every((row) => row.ok) ? 0 : 1);
