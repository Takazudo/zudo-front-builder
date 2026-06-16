/**
 * Playwright configuration for the WebKit back-navigation harness.
 *
 * LOCAL-HEAVY (T4) — Mac only. NOT wired into CI or `pnpm test`.
 *
 * Prerequisites:
 *   1. Install the WebKit browser once per machine:
 *        npx playwright install webkit
 *   2. Build the runtime ESM:
 *        pnpm -C packages/zfb-runtime build
 *
 * Run via the workspace script (handles build + serve + test):
 *   pnpm test:webkit-back
 *
 * Or run directly from the repo root after a manual build:
 *   npx playwright test --config tests/webkit-back-history/playwright.config.mjs
 */

// @ts-check
import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

// Absolute path to the repo root so webServer.cwd is unambiguous regardless
// of where `playwright test` is invoked from.
const REPO_ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");

export default defineConfig({
  // Test files in this harness directory only.
  testDir: "./",
  testMatch: "**/*.webkit.spec.mjs",

  // Each test case gets a clean browser context; no shared state.
  fullyParallel: false,

  // No retries: this is a regression harness. Flake means a real issue.
  retries: 0,

  // One worker to keep history state deterministic across serial cases.
  workers: 1,

  reporter: "list",

  use: {
    // Base URL set by webServer below.
    baseURL: "http://localhost:4321",
    // Capture trace on first retry (won't fire since retries: 0, but good default).
    trace: "on-first-retry",
  },

  projects: [
    {
      name: "webkit",
      use: {
        ...devices["Desktop Safari"],
        // Emulate a mobile viewport to better approximate iOS back-swipe conditions.
        // The hasUAVisualTransition flag is the key signal — not strict pixel dimensions.
        viewport: { width: 390, height: 844 },
      },
    },
  ],

  // Build + start the static fixture server before running tests.
  // `reuseExistingServer: false` ensures a fresh server every run.
  webServer: {
    command: "node tests/webkit-back-history/serve-fixture.mjs 4321",
    url: "http://localhost:4321",
    // Matches the "READY …" line printed by serve-fixture.mjs.
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
  },
});
