import { defineConfig, devices } from "@playwright/test";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const PORT = Number.parseInt(process.env.WIND_DOCS_PREVIEW_PORT ?? "4333", 10);
const BASE_PATH = normalizeBase(process.env.WIND_DOCS_BASE ?? "/");
const BASE_URL = `http://127.0.0.1:${PORT}${BASE_PATH}`;
const DIST_DIR = process.env.WIND_DOCS_DIST ?? join(REPO_ROOT, "docs/dist");

function normalizeBase(value) {
  if (value === "" || value === "/") return "/";
  const normalized = `/${value.replace(/^\/+|\/+$/g, "")}/`;
  if (normalized.includes("/../") || normalized.includes("/./")) {
    throw new Error(`WIND_DOCS_BASE must be a local URL path, received ${JSON.stringify(value)}`);
  }
  return normalized;
}

export default defineConfig({
  testDir: "./",
  testMatch: "**/*.chromium.spec.mjs",
  fullyParallel: true,
  workers: process.env.CI ? 2 : undefined,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  reporter: "list",
  outputDir: join(tmpdir(), "zfb-wind-doc-preview-results"),
  timeout: 90000,
  use: {
    baseURL: BASE_URL,
    browserName: "chromium",
    colorScheme: "light",
    viewport: { width: 1440, height: 1000 },
    trace: {
      mode: "retain-on-failure",
      snapshots: false,
      screenshots: true,
      sources: true,
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 1000 } },
    },
  ],
  webServer: {
    command: `node tests/docs-wind-preview/serve-dist.mjs ${PORT}`,
    url: BASE_URL,
    reuseExistingServer: false,
    cwd: REPO_ROOT,
    timeout: 15000,
    env: {
      WIND_DOCS_BASE: BASE_PATH,
      WIND_DOCS_DIST: DIST_DIR,
    },
  },
});
