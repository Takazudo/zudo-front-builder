import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";

export default defineConfig({
  testDir: fileURLToPath(new URL(".", import.meta.url)),
  testMatch: "*.chromium.spec.mjs",
  workers: 1,
  fullyParallel: false,
  retries: 0,
  forbidOnly: true,
  reporter: "list",
  preserveOutput: "always",
  outputDir: fileURLToPath(
    new URL("../../test-results/cascade-decision-experiment", import.meta.url),
  ),
  use: {
    ...devices["Desktop Chrome"],
    trace: "retain-on-failure",
    launchOptions: process.env.CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.CHROMIUM_EXECUTABLE_PATH }
      : {},
  },
});
