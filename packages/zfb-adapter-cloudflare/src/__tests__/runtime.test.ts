import { describe, expect, it } from "vitest";

import {
  getCloudflareContext,
  runWithCloudflareContext,
  type CloudflareExecutionContext,
} from "../index.js";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function stubCtx(): CloudflareExecutionContext {
  return {
    waitUntil: () => undefined,
    passThroughOnException: () => undefined,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("runWithCloudflareContext / getCloudflareContext", () => {
  it("exposes env and ctx synchronously inside the run scope", () => {
    const env = { ANTHROPIC_API_KEY: "secret-token" };
    const ctx = stubCtx();
    const request = new Request("https://example.test/api/foo");

    const observed = runWithCloudflareContext({ env, ctx, request }, () => {
      return getCloudflareContext<typeof env>();
    });

    expect(observed.env.ANTHROPIC_API_KEY).toBe("secret-token");
    // Identity, not deep equality — same Request instance threads through.
    expect(observed.request).toBe(request);
    expect(observed.ctx).toBe(ctx);
  });

  it("preserves the context across awaits", async () => {
    const env = { KEY: "v" };
    const ctx = stubCtx();
    const request = new Request("https://example.test/");

    const observed = await runWithCloudflareContext({ env, ctx, request }, async () => {
      await new Promise((r) => setTimeout(r, 0));
      await Promise.resolve();
      return getCloudflareContext<typeof env>();
    });

    expect(observed.env.KEY).toBe("v");
  });

  it("throws a clear error when called outside a run scope", () => {
    expect(() => getCloudflareContext()).toThrow(/Cloudflare request scope/);
  });

  it("isolates concurrent requests via AsyncLocalStorage", async () => {
    // The whole point of using AsyncLocalStorage instead of globalThis
    // is that two overlapping requests must each see their own env.
    // We interleave the two by yielding inside the inner thunks.
    const ctx = stubCtx();

    const reqA = runWithCloudflareContext(
      { env: { tag: "A" }, ctx, request: new Request("https://a.test/") },
      async () => {
        await Promise.resolve();
        const seenBefore = getCloudflareContext<{ tag: string }>().env.tag;
        await new Promise((r) => setTimeout(r, 5));
        const seenAfter = getCloudflareContext<{ tag: string }>().env.tag;
        return [seenBefore, seenAfter];
      },
    );
    const reqB = runWithCloudflareContext(
      { env: { tag: "B" }, ctx, request: new Request("https://b.test/") },
      async () => {
        await Promise.resolve();
        const seenBefore = getCloudflareContext<{ tag: string }>().env.tag;
        await new Promise((r) => setTimeout(r, 1));
        const seenAfter = getCloudflareContext<{ tag: string }>().env.tag;
        return [seenBefore, seenAfter];
      },
    );

    const [a, b] = await Promise.all([reqA, reqB]);
    expect(a).toEqual(["A", "A"]);
    expect(b).toEqual(["B", "B"]);
  });
});
