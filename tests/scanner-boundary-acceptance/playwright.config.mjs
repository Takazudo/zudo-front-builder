import { defineConfig } from "@playwright/test";
import { engineProject } from "../webkit-token/engine.mjs";
import { join } from "node:path";

const REPO_ROOT = process.cwd();
const PORT = 4324;

export default defineConfig({
  testDir: REPO_ROOT,
  testMatch: "registry.chromium.spec.mjs",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: "list",
  outputDir: join(REPO_ROOT, "test-results", "scanner-boundary-acceptance"),
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "on-first-retry",
  },
  projects: [engineProject],
  webServer: {
    command: `node tests/scanner-boundary-acceptance/serve-dist.mjs ${PORT}`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15_000,
  },
});
