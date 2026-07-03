/**
 * Playwright configuration for the router-chromium L4 smoke suite.
 *
 * CI + LOCAL. Unlike the WebKit T4 harness (Mac-only, not in CI), Chromium
 * downloads fine on ubuntu runners, so this suite runs in CI on router-affecting
 * PRs (see .github/workflows/router-chromium.yml).
 *
 * Prerequisites:
 *   1. Build the runtime ESM:      pnpm -C packages/zfb-runtime build
 *   2. Install Chromium once:      pnpm exec playwright install chromium
 *
 * Run via the workspace script (handles build + serve + test):
 *   pnpm test:router-chromium
 *
 * Or directly from the repo root after a manual build:
 *   pnpm exec playwright test --config tests/router-chromium/playwright.config.mjs
 */

// @ts-check
import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// Absolute path to the repo root so webServer.cwd and outputDir are unambiguous
// regardless of where `playwright test` is invoked from.
const REPO_ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");

const PORT = 4322;

export default defineConfig({
  testDir: "./",
  testMatch: "**/*.chromium.spec.mjs",

  // Serial + single worker: history/scroll traversal is deterministic when one
  // context runs at a time. The suite is small (< 3 min target), so there is no
  // parallelism to trade away.
  fullyParallel: false,
  workers: 1,

  // Fail the run if a spec is accidentally left `.only` in CI.
  forbidOnly: !!process.env.CI,

  // Deflaking discipline (issue #1346): 0 retries locally so a flake fails loudly
  // during the burn-in; 1 retry in CI with a trace captured on that retry.
  retries: process.env.CI ? 1 : 0,

  reporter: "list",

  // Traces (and any other per-test artifacts) land here. The CI workflow uploads
  // this directory on failure — traces live in test-results/, NOT in
  // playwright-report/ (issue #1346 acceptance criteria).
  outputDir: join(REPO_ROOT, "test-results"),

  use: {
    baseURL: `http://localhost:${PORT}`,
    // Only capture a trace when a test is retried (CI-only, since retries: 0
    // locally). Keeps the happy path free of tracing overhead.
    trace: "on-first-retry",
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

  // Start the static fixture server before the tests. `reuseExistingServer:
  // false` guarantees a fresh server each run (important for the local 20x
  // burn-in loop, where a stale server must never bleed state across runs).
  webServer: {
    command: `node tests/router-chromium/serve-fixture.mjs ${PORT}`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
  },
});
