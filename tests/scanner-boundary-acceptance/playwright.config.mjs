import { defineConfig, devices } from "@playwright/test";
import { join } from "node:path";

const REPO_ROOT = process.cwd();
const PORT = 4324;

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
  projects: [
    {
      name: ENGINE,
      use: { ...ENGINE_DEVICES[ENGINE] },
    },
  ],
  webServer: {
    command: `node tests/scanner-boundary-acceptance/serve-dist.mjs ${PORT}`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15_000,
  },
});
