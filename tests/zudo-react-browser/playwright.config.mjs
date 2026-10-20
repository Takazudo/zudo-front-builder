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

// ZFB_BROWSER_ENGINE=webkit runs the same specs in WebKit instead of Chromium
// (#4231 item 2, `pnpm test:webkit-token`). Unset, the project is unchanged.
const ENGINE = process.env.ZFB_BROWSER_ENGINE ?? "chromium";
const ENGINE_DEVICES = {
  chromium: devices["Desktop Chrome"],
  webkit: devices["Desktop Safari"],
};
if (!(ENGINE in ENGINE_DEVICES)) {
  throw new Error(`unsupported ZFB_BROWSER_ENGINE '${ENGINE}' (expected chromium or webkit)`);
}

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
      name: ENGINE,
      use: { ...ENGINE_DEVICES[ENGINE] },
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
