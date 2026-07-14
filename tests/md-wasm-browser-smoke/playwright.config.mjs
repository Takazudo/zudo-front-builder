/**
 * Chromium confirmation for a real zfb production build that lazily imports
 * the packed md-wasm package. Building/staging remains an explicit CI step so
 * this configuration only serves the already-built dist/ tree.
 */

import { defineConfig, devices } from "@playwright/test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const testRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(testRoot, "..", "..");
const port = 4331;

export default defineConfig({
  testDir: "./",
  testMatch: "**/*.chromium.spec.mjs",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: "list",
  outputDir: join(repoRoot, "test-results"),
  use: {
    baseURL: `http://localhost:${port}`,
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: `node tests/md-wasm-browser-smoke/serve-dist.mjs ${port}`,
    cwd: repoRoot,
    url: `http://localhost:${port}`,
    reuseExistingServer: false,
    timeout: 15000,
  },
});
