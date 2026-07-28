// Smoke tests for the `strictBrokenLinks` config type and `defineConfig`
// identity round-trip.

import { describe, expect, it } from "vitest";
import { defineConfig } from "../config.js";
import type { ZfbConfig } from "../config.js";

describe("strictBrokenLinks config type + defineConfig round-trip", () => {
  it("defineConfig accepts strictBrokenLinks: true", () => {
    const cfg = defineConfig({ strictBrokenLinks: true });
    expect(cfg.strictBrokenLinks).toBe(true);
  });

  it("defineConfig accepts strictBrokenLinks: false", () => {
    const cfg = defineConfig({ strictBrokenLinks: false });
    expect(cfg.strictBrokenLinks).toBe(false);
  });

  it("defineConfig accepts absent strictBrokenLinks field", () => {
    const cfg = defineConfig({});
    expect(cfg.strictBrokenLinks).toBeUndefined();
  });

  it("ZfbConfig includes optional strictBrokenLinks", () => {
    const input: ZfbConfig = { strictBrokenLinks: true };
    expect(defineConfig(input)).toBe(input);
  });
});
