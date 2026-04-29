// Integration test for the build-time CLI helper.
//
// Validates two things:
//
// 1. The wrapper string exported by `bin/cli.mjs` (re-exported from
//    `src/worker-wrapper.mjs`) matches the canonical constant in
//    `src/build.ts`. Both share a single source of truth — this test
//    guards against accidental divergence if the import chain breaks.
//
// 2. End-to-end: running `emitWorker` against a synthetic input bundle
//    produces a `_worker.js` whose `default.fetch` correctly threads
//    `(env, ctx)` into the inner bundle's view via AsyncLocalStorage.
//    This is the "synthetic Request + env stub" check the task brief
//    asks for in lieu of a real wrangler dev run.

import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import { emitWorker, WORKER_WRAPPER_SOURCE as TS_WRAPPER } from "../build.js";
// CLI helper is a sibling .mjs — Node 22 resolves the .mjs ESM directly.
// The path is computed at test time so this file is portable across
// pnpm-installed and workspace-relative layouts. The `.mjs` ships
// without a `.d.mts` companion (it is consumed only via `bin`, not as
// a TS module), so we narrow the import shape ourselves and silence
// TS's "no declaration file" complaint.
// @ts-expect-error: bin/cli.mjs has no declaration file; we narrow below.
import { WORKER_WRAPPER_SOURCE as MJS_WRAPPER_RAW } from "../../bin/cli.mjs";
const MJS_WRAPPER: string = MJS_WRAPPER_RAW as string;

let scratchDirs: string[] = [];

afterEach(async () => {
  for (const d of scratchDirs) {
    await rm(d, { recursive: true, force: true });
  }
  scratchDirs = [];
});

async function scratch(): Promise<string> {
  const d = await mkdtemp(join(tmpdir(), "zfb-adapter-cf-"));
  scratchDirs.push(d);
  return d;
}

describe("CLI / emitWorker", () => {
  it("WORKER_WRAPPER_SOURCE re-exported by bin/cli.mjs matches the build.ts constant", () => {
    // Both src/build.ts and bin/cli.mjs import from the canonical
    // src/worker-wrapper.mjs, so they must be the same string.
    // This guards against import-chain breakage.
    expect(MJS_WRAPPER).toBe(TS_WRAPPER);
  });

  it("emits _worker.js and _zfb_inner.mjs side-by-side", async () => {
    const dir = await scratch();
    const inputPath = join(dir, "bundle.mjs");
    await writeFile(
      inputPath,
      `export default { fetch: async () => new Response("hello", { status: 200 }) };\n`,
      "utf8",
    );

    const out = await emitWorker({
      inputBundlePath: inputPath,
      outdir: join(dir, "dist"),
    });

    expect(out.workerPath).toBe(join(dir, "dist", "_worker.js"));
    expect(out.innerBundlePath).toBe(join(dir, "dist", "_zfb_inner.mjs"));

    const wrapperBody = await readFile(out.workerPath, "utf8");
    const innerBody = await readFile(out.innerBundlePath, "utf8");
    expect(wrapperBody).toBe(TS_WRAPPER);
    expect(innerBody).toContain("hello");
  });

  it("threads env/ctx from the wrapper into the inner bundle's request scope", async () => {
    // Build a synthetic inner bundle that uses the same globalThis-key
    // AsyncLocalStorage trick the user-facing module uses, so we can
    // observe the env value from inside the inner bundle as the
    // wrapper invokes its fetch handler.
    //
    // In production the same registry is set up by
    // @takazudo/zfb-adapter-cloudflare's getCloudflareContext(); here
    // we inline the read-side so the test is self-contained and does
    // not depend on bundling the user package into the inner.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    await writeFile(
      inputPath,
      `import { AsyncLocalStorage } from "node:async_hooks";
const STORAGE_KEY = "__zfb_cf_adapter_als__";
function getStore() {
  const g = globalThis;
  let als = g[STORAGE_KEY];
  if (!als) { als = new AsyncLocalStorage(); g[STORAGE_KEY] = als; }
  return als.getStore();
}
export default {
  async fetch(request) {
    const store = getStore();
    const env = store?.env ?? {};
    const ctxKind = typeof store?.ctx?.waitUntil;
    return new Response(
      JSON.stringify({
        url: request.url,
        token: env.ANTHROPIC_API_KEY ?? null,
        ctxKind,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  },
};
`,
      "utf8",
    );

    const out = await emitWorker({
      inputBundlePath: inputPath,
      outdir: join(dir, "dist"),
    });

    // Import the produced _worker.js as ESM and drive its fetch with a
    // synthetic Request + env + ctx. This is the integration check the
    // brief asks for in lieu of running the bundle under wrangler.
    const worker = (await import(pathToFileURL(out.workerPath).href)) as {
      default: {
        fetch: (
          request: Request,
          env: Record<string, unknown>,
          ctx: { waitUntil: (p: Promise<unknown>) => void; passThroughOnException: () => void },
        ) => Promise<Response>;
      };
    };

    let waitUntilCalled = 0;
    const ctx = {
      waitUntil: () => {
        waitUntilCalled += 1;
      },
      passThroughOnException: () => undefined,
    };
    const env = { ANTHROPIC_API_KEY: "sk-test-1234" };
    const request = new Request("https://worker.test/api/foo");

    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      url: string;
      token: string | null;
      ctxKind: string;
    };

    expect(body.url).toBe("https://worker.test/api/foo");
    // env passthrough — the headline acceptance check.
    expect(body.token).toBe("sk-test-1234");
    // ctx surface is structurally the workerd ExecutionContext shape.
    expect(body.ctxKind).toBe("function");
    // The inner did not invoke waitUntil; this asserts we passed our
    // ctx in (rather than e.g. a constructor-default empty object).
    expect(waitUntilCalled).toBe(0);
  });

  it("falls through to env.ASSETS when the inner bundle returns 404", async () => {
    // CF Pages advanced-mode contract: when _worker.js is present, every
    // request hits the worker. Static assets (search-index.json,
    // post-build llms.txt, generated SSG HTML at canonical trailing-slash
    // URLs) are NOT served unless the worker explicitly delegates.
    // The wrapper must fall through to env.ASSETS on 404 so those keep
    // working in production.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    // Inner bundle that always 404s — simulates a request whose URL
    // matches no SSR route.
    await writeFile(
      inputPath,
      `export default {
  async fetch() {
    return new Response("inner says 404", { status: 404 });
  },
};
`,
      "utf8",
    );

    const out = await emitWorker({
      inputBundlePath: inputPath,
      outdir: join(dir, "dist"),
    });
    const worker = (await import(pathToFileURL(out.workerPath).href)) as {
      default: {
        fetch: (request: Request, env: Record<string, unknown>, ctx: unknown) => Promise<Response>;
      };
    };

    const ctx = {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
    };

    let assetsCalls = 0;
    const env = {
      ASSETS: {
        fetch: async (req: Request) => {
          assetsCalls += 1;
          return new Response(`asset for ${new URL(req.url).pathname}`, {
            status: 200,
            headers: { "content-type": "text/plain" },
          });
        },
      },
    };
    const request = new Request("https://worker.test/search-index.json");

    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("asset for /search-index.json");
    expect(assetsCalls).toBe(1);
  });

  it("does NOT fall through to env.ASSETS when the inner returns a non-404", async () => {
    // Guards against the wrapper accidentally re-routing legitimate
    // dynamic responses (200, 4xx other than 404, 5xx, etc.) to the
    // static asset server.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    await writeFile(
      inputPath,
      `export default {
  async fetch() {
    return new Response('{"error":"bad request"}', {
      status: 400,
      headers: { "content-type": "application/json" },
    });
  },
};
`,
      "utf8",
    );

    const out = await emitWorker({
      inputBundlePath: inputPath,
      outdir: join(dir, "dist"),
    });
    const worker = (await import(pathToFileURL(out.workerPath).href)) as {
      default: {
        fetch: (request: Request, env: Record<string, unknown>, ctx: unknown) => Promise<Response>;
      };
    };

    let assetsCalls = 0;
    const env = {
      ASSETS: {
        fetch: async () => {
          assetsCalls += 1;
          return new Response("should not be called", { status: 200 });
        },
      },
    };
    const ctx = {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
    };
    const request = new Request("https://worker.test/api/foo", { method: "POST" });

    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(400);
    expect(await response.json()).toEqual({ error: "bad request" });
    expect(assetsCalls).toBe(0);
  });
});
