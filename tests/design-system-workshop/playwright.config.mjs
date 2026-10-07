import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  workers: 1,
  retries: 0,
  timeout: 60000,
  reporter: "list",
  outputDir: "/tmp/zfb-design-workshop-results",
  use: {
    baseURL: "http://127.0.0.1:4340",
    viewport: { width: 1440, height: 1000 },
    launchOptions: process.env.ZFB_CHROMIUM_PATH
      ? { executablePath: process.env.ZFB_CHROMIUM_PATH }
      : {},
    permissions: ["clipboard-read", "clipboard-write"],
    trace: "retain-on-failure",
  },
  webServer: {
    cwd: new URL("../..", import.meta.url).pathname,
    command: "node tests/docs-wind-preview/serve-dist.mjs 4340",
    url: "http://127.0.0.1:4340",
    reuseExistingServer: false,
    timeout: 15000,
  },
});
