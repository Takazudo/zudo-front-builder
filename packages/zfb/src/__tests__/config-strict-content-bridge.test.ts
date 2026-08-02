// Smoke tests for the `strictContentBridge` config type and `defineConfig`
// identity round-trip (issue #2220, epic #2216 Wave 4).

import { describe, expect, it } from "vitest";
import { defineConfig } from "../config.js";
import type { ZfbConfig } from "../config.js";

describe("strictContentBridge config type + defineConfig round-trip", () => {
  it("defineConfig accepts strictContentBridge: true", () => {
    const cfg = defineConfig({ strictContentBridge: true });
    expect(cfg.strictContentBridge).toBe(true);
  });

  it("defineConfig accepts strictContentBridge: false", () => {
    const cfg = defineConfig({ strictContentBridge: false });
    expect(cfg.strictContentBridge).toBe(false);
  });

  it("defineConfig accepts absent strictContentBridge field", () => {
    const cfg = defineConfig({});
    expect(cfg.strictContentBridge).toBeUndefined();
  });

  it("ZfbConfig includes optional strictContentBridge", () => {
    const input: ZfbConfig = { strictContentBridge: true };
    expect(defineConfig(input)).toBe(input);
  });
});
