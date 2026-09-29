import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(__dirname, "..", "..");
const PORT = 4341;

export default defineConfig({
  testDir: __dirname,
  testMatch: /.*\.chromium\.spec\.mjs/,
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
    command: `node tests/wind-computed-style/serve-fixture.mjs ${PORT}`,
    url: `http://localhost:${PORT}/fixtures/w-a01/index.html`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
  },
});
