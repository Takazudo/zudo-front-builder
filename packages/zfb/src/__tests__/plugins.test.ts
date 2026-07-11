import { describe, expect, it } from "vitest";
import { definePlugin } from "../plugins.js";
import type { ZfbDevMiddlewareContext, ZfbPlugin } from "../plugins.js";

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

  it("preserves the previewMiddleware hook field (#1542)", () => {
    // Mirrors the devMiddleware coverage above — previewMiddleware is a
    // distinct hook (not a devMiddleware alias), so a plugin declares it
    // separately with the same register-context shape.
    const plugin = definePlugin({
      name: "preview-hook",
      previewMiddleware: ({ register }) => {
        register("/test", () => ({ status: 200 }));
      },
    });
    expect(plugin.name).toBe("preview-hook");
    expect(typeof plugin.previewMiddleware).toBe("function");
  });

  it("allows a plugin to register the same handler under both devMiddleware and previewMiddleware", () => {
    // #1542 baked decision: a plugin wanting both dev AND preview
    // coverage must register the same handler under both hooks
    // explicitly — there is no automatic devMiddleware-reuse. The two
    // context types are structurally identical, so one handler value
    // satisfies both hook fields' types.
    const handler = ({ register }: ZfbDevMiddlewareContext) => {
      register("/shared", () => ({ status: 200 }));
    };
    const plugin = definePlugin({
      name: "shared-hooks",
      devMiddleware: handler,
      previewMiddleware: handler,
    });
    expect(plugin.devMiddleware).toBe(plugin.previewMiddleware);
    expect(typeof plugin.previewMiddleware).toBe("function");
  });
});
