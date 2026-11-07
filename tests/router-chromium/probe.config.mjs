// Investigation-only config (refs #4607): runs `*.probe.mjs` measurement specs
// against the same fixture server, outside the CI suite's `*.chromium.spec.mjs`
// match. Run: pnpm exec playwright test --config tests/router-chromium/probe.config.mjs
// @ts-check
import { defineConfig } from "@playwright/test";
import base from "./playwright.config.mjs";

export default defineConfig({
  ...base,
  testMatch: "**/*.probe.mjs",
  retries: 0,
  workers: 1,
});
