/**
 * Playwright configuration for the built-site-smoke L4 suite (issue #1401).
 *
 * Structural guard for the #1385 bug class (an emitted client bundle that
 * throws at hydration) — the one gap every other tier leaves open:
 *   - router-chromium.yml drives zfb-runtime fixtures, not `zfb build` output;
 *   - the Rust dev-server e2es are HTTP/process-level, never a real browser;
 *   - the node-free smokes (tests/smoke/) assert dist/ file CONTENT via
 *     grep, never execute the emitted JS.
 *
 * Mirrors tests/router-chromium/playwright.config.mjs's shape. The key
 * difference: router-chromium builds packages/zfb-runtime itself before
 * starting; this suite instead expects the CI workflow to have already run
 * a real `zfb build` against fixture-site/ (see
 * .github/workflows/node-free-smoke.yml's `built-site-browser-smoke` job) —
 * building requires the platform-specific `zfb` binary, not a pnpm script,
 * so it cannot be folded into this config's `webServer.command`.
 *
 * Prerequisites to run locally:
 *   1. Build fixture-site/dist/:  <path-to-zfb-binary> build   (cwd: tests/built-site-smoke/fixture-site/)
 *   2. Install Chromium once:     pnpm exec playwright install chromium
 *
 * Then: pnpm exec playwright test --config tests/built-site-smoke/playwright.config.mjs
 */

// @ts-check
import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// Absolute path to the repo root so webServer.cwd and outputDir are unambiguous
// regardless of where `playwright test` is invoked from.
const REPO_ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");

const PORT = 4323;

export default defineConfig({
  testDir: "./",
  testMatch: "**/*.chromium.spec.mjs",

  // Single spec, single assertion flow — no parallelism to trade away.
  fullyParallel: false,
  workers: 1,

  // Fail the run if a spec is accidentally left `.only` in CI.
  forbidOnly: !!process.env.CI,

  // Deflaking discipline (matches router-chromium.yml): 0 retries locally so
  // a flake fails loudly during authoring; 1 retry in CI with a trace
  // captured on that retry.
  retries: process.env.CI ? 1 : 0,

  reporter: "list",

  // Shared with router-chromium's outputDir — both are gitignored
  // (/test-results/) and never run in the same job, so there is no
  // collision risk.
  outputDir: join(REPO_ROOT, "test-results"),

  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "on-first-retry",
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

  // Serves the pre-built fixture-site/dist/ — does NOT build it (see header
  // comment above). `reuseExistingServer: false` guarantees a fresh server
  // each run.
  webServer: {
    command: `node tests/built-site-smoke/serve-dist.mjs ${PORT}`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
  },
});
