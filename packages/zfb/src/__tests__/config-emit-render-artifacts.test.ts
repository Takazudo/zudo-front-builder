// Smoke tests for the `emitRenderArtifacts` config type and `defineConfig`
// identity round-trip (Render Artifact Export epic #2421, sub-issue #2422).

import { describe, expect, it } from "vitest";
import { defineConfig } from "../config.js";
import type { ZfbConfig } from "../config.js";

describe("emitRenderArtifacts config type + defineConfig round-trip", () => {
  it("defineConfig accepts emitRenderArtifacts: true", () => {
    const cfg = defineConfig({ emitRenderArtifacts: true });
    expect(cfg.emitRenderArtifacts).toBe(true);
  });

  it("defineConfig accepts emitRenderArtifacts: false", () => {
    const cfg = defineConfig({ emitRenderArtifacts: false });
    expect(cfg.emitRenderArtifacts).toBe(false);
  });

  it("defineConfig accepts absent emitRenderArtifacts field", () => {
    const cfg = defineConfig({});
    expect(cfg.emitRenderArtifacts).toBeUndefined();
  });

  it("ZfbConfig includes optional emitRenderArtifacts", () => {
    const input: ZfbConfig = { emitRenderArtifacts: true };
    expect(defineConfig(input)).toBe(input);
  });
});
