import { describe, expect, it } from "vitest";
import { definePreset } from "../config.js";
import type { CollectionDef } from "../config.js";

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

  it("preserves an inner preset's source_package when composed by an outer preset", () => {
    // An outer preset spreads the plugins of an inner definePreset-returned
    // preset. The inner plugin's provenance must NOT be clobbered by the outer
    // package name, or the inner preset's relative plugins would resolve
    // against the wrong package.
    const inner = definePreset("@scope/inner-preset", {
      plugins: [{ name: "./inner-plugin.mjs" }],
    });
    const outer = definePreset("@scope/outer-preset", {
      plugins: [{ name: "./outer-plugin.mjs" }, ...(inner.plugins ?? [])],
    });
    expect(outer.plugins).toEqual([
      { name: "./outer-plugin.mjs", source_package: "@scope/outer-preset" },
      { name: "./inner-plugin.mjs", source_package: "@scope/inner-preset" },
    ]);
  });
});

describe("CollectionDef.allowOutsideRoot", () => {
  it("is an optional field that defaults to undefined", () => {
    const collection: CollectionDef = { name: "blog", path: "content/blog" };
    expect(collection.allowOutsideRoot).toBeUndefined();
  });

  it("passes through definePreset unchanged (definePreset only stamps plugins)", () => {
    // A preset can carry a collection with `allowOutsideRoot: true`
    // pointing outside its own package — the flag must survive
    // `definePreset` untouched so the Rust loader sees exactly what the
    // preset author declared.
    const collection: CollectionDef = {
      name: "shared-notes",
      path: "../shared-notes",
      allowOutsideRoot: true,
    };
    const result = definePreset("my-preset-pkg", {
      collections: [collection],
      plugins: [{ name: "plugin-a" }],
    });
    expect(result.collections).toEqual([collection]);
  });
});
