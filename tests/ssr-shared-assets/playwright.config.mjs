import { defineConfig } from "@playwright/test";
import { join } from "node:path";
import { engineProject } from "../webkit-token/engine.mjs";

const REPO_ROOT = process.cwd();
export const ROOT_PORT = 4331;
export const SUBPATH_PORT = 4332;

// Consumes the builds `ssr_shared_assets_e2e` preserves under
// target/ssr-shared-assets/ (#4887).
export default defineConfig({
  testDir: REPO_ROOT,
  testMatch: "tests/ssr-shared-assets/*.chromium.spec.mjs",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: "list",
  outputDir: join(REPO_ROOT, "test-results", "ssr-shared-assets"),
  use: { trace: "on-first-retry" },
  projects: [engineProject],
  webServer: [
    {
      command: `node tests/ssr-shared-assets/serve.mjs ${ROOT_PORT} root`,
      url: `http://127.0.0.1:${ROOT_PORT}/plain`,
      reuseExistingServer: false,
      cwd: REPO_ROOT,
      timeout: 15_000,
    },
    {
      command: `node tests/ssr-shared-assets/serve.mjs ${SUBPATH_PORT} subpath /sub/`,
      url: `http://127.0.0.1:${SUBPATH_PORT}/sub/plain`,
      reuseExistingServer: false,
      cwd: REPO_ROOT,
      timeout: 15_000,
    },
  ],
});
