/** Local Chromium network confirmation for W-A06 (#3269). */

import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const REPO_ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const PORT = 4332;

export default defineConfig({
  testDir: "./",
  testMatch: "**/*.chromium.spec.mjs",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
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
    command: `node tests/wind-real-build/serve-dist.mjs ${PORT}`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
  },
});
