// Smoke tests for the `PrefetchConfig` type addition and `defineConfig`
// round-trip (#277).
//
// These tests are mostly compile-time (TypeScript shape) but also pin the
// runtime identity of `defineConfig` — if the field name is misspelled or the
// type changes shape, the test fails.

import { describe, expect, it } from "vitest";
import { defineConfig } from "../config.js";
import type { PrefetchConfig, ZfbConfig } from "../config.js";

describe("PrefetchConfig type + defineConfig round-trip", () => {
  it("defineConfig accepts prefetch: { disabled: true } without type error", () => {
    // TypeScript compile-time check that the field exists in ZfbConfig.
    // At runtime, defineConfig is the identity function — the returned
    // value must equal the input object reference.
    const cfg = defineConfig({ prefetch: { disabled: true } });
    expect(cfg.prefetch?.disabled).toBe(true);
  });

  it("defineConfig accepts prefetch: { disabled: false }", () => {
    const cfg = defineConfig({ prefetch: { disabled: false } });
    expect(cfg.prefetch?.disabled).toBe(false);
  });

  it("defineConfig accepts prefetch: {} (disabled omitted)", () => {
    const cfg = defineConfig({ prefetch: {} });
    expect(cfg.prefetch).toEqual({});
    expect(cfg.prefetch?.disabled).toBeUndefined();
  });

  it("defineConfig accepts absent prefetch field", () => {
    const cfg = defineConfig({});
    expect(cfg.prefetch).toBeUndefined();
  });

  it("PrefetchConfig type is assignable as expected", () => {
    // Structural type check: an object with only `disabled` is a valid
    // PrefetchConfig, and ZfbConfig.prefetch is Optional<PrefetchConfig>.
    const pc: PrefetchConfig = { disabled: true };
    const zc: ZfbConfig = { prefetch: pc };
    expect(zc.prefetch?.disabled).toBe(true);
  });

  it("defineConfig returns the identical object reference (identity helper)", () => {
    // defineConfig must remain a no-op identity function — no cloning.
    const input: ZfbConfig = { prefetch: { disabled: true } };
    expect(defineConfig(input)).toBe(input);
  });
});
