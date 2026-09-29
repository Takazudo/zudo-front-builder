/**
 * Playwright configuration for the packed-SDK zudo-react L4 smoke suite.
 *
 * `pnpm test:zudo-react-browser` stages the published package, builds the TSX
 * fixtures and runs this config. Install Chromium once with
 * `pnpm exec playwright install chromium` for local use.
 */

// @ts-check
import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const REPO_ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const PORT = 4342;

export default defineConfig({
  testDir: "./",
  testMatch: "**/*.chromium.spec.mjs",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: "list",
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
  webServer: {
    command: `node tests/zudo-react-browser/serve-fixture.mjs ${PORT}`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
  },
});
