// Smoke tests for the `minifyHtml` config type and `defineConfig`
// identity round-trip.

import { describe, expect, it } from "vitest";
import { defineConfig } from "../config.js";
import type { ZfbConfig } from "../config.js";

describe("minifyHtml config type + defineConfig round-trip", () => {
  it("defineConfig accepts minifyHtml: true", () => {
    const cfg = defineConfig({ minifyHtml: true });
    expect(cfg.minifyHtml).toBe(true);
  });

  it("defineConfig accepts minifyHtml: false", () => {
    const cfg = defineConfig({ minifyHtml: false });
    expect(cfg.minifyHtml).toBe(false);
  });

  it("defineConfig accepts absent minifyHtml field", () => {
    const cfg = defineConfig({});
    expect(cfg.minifyHtml).toBeUndefined();
  });

  it("ZfbConfig includes optional minifyHtml", () => {
    const input: ZfbConfig = { minifyHtml: true };
    expect(defineConfig(input)).toBe(input);
  });
});
