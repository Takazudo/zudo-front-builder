import { describe, expect, it } from "vitest";
import { definePreset } from "../config.js";

describe("definePreset", () => {
  it("stamps each object plugin entry with source_package", () => {
    const result = definePreset("my-preset-pkg", {
      plugins: [{ name: "plugin-a" }, { name: "plugin-b", options: { key: "val" } }],
    });
    expect(result.plugins).toEqual([
      { name: "plugin-a", source_package: "my-preset-pkg" },
      { name: "plugin-b", options: { key: "val" }, source_package: "my-preset-pkg" },
    ]);
  });

  it("passes through non-plugin fields unchanged", () => {
    const result = definePreset("my-preset-pkg", {
      outDir: "build",
      framework: "react",
      plugins: [{ name: "p" }],
    });
    expect(result.outDir).toBe("build");
    expect(result.framework).toBe("react");
  });

  it("returns config unchanged when plugins is absent", () => {
    const config = { outDir: "dist", framework: "preact" as const };
    const result = definePreset("my-preset-pkg", config);
    expect(result).toBe(config);
  });

  it("returns config unchanged when plugins is an empty array", () => {
    const config = { plugins: [] };
    const result = definePreset("my-preset-pkg", config);
    // plugins array is mapped over; result is a new object but semantically identical
    expect(result.plugins).toEqual([]);
  });

  it("does not mutate the original config object", () => {
    const original = {
      plugins: [{ name: "plugin-a" }],
    };
    const originalPlugin = original.plugins[0];
    definePreset("my-preset-pkg", original);
    // original plugin object should be unchanged
    expect(original.plugins[0]).toBe(originalPlugin);
    expect((original.plugins[0] as Record<string, unknown>)["source_package"]).toBeUndefined();
  });
});
