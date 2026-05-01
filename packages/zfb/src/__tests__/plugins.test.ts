import { describe, expect, it } from "vitest";
import { definePlugin } from "../plugins.js";
import type { ZfbPlugin } from "../plugins.js";

describe("definePlugin", () => {
  it("returns the supplied plugin object verbatim (identity helper)", () => {
    // The whole point of `definePlugin` is to add types without a
    // runtime cost. The runtime semantics are an identity function.
    const plugin: ZfbPlugin = {
      name: "test",
      preBuild() {},
    };
    expect(definePlugin(plugin)).toBe(plugin);
  });

  it("preserves all hook fields (preBuild, postBuild, devMiddleware)", () => {
    const plugin = definePlugin({
      name: "all-hooks",
      preBuild: async () => {},
      postBuild: async () => {},
      devMiddleware: ({ register }) => {
        register("/test", () => ({ status: 200 }));
      },
    });
    expect(plugin.name).toBe("all-hooks");
    expect(typeof plugin.preBuild).toBe("function");
    expect(typeof plugin.postBuild).toBe("function");
    expect(typeof plugin.devMiddleware).toBe("function");
  });
});
