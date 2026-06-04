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
import {
  WORKER_WRAPPER_SOURCE as MJS_WRAPPER_RAW,
  emitWorker as cliEmitWorker,
} from "../../bin/cli.mjs";
const MJS_WRAPPER: string = MJS_WRAPPER_RAW as string;
const CLI_EMIT_WORKER: (input: {
  inputBundlePath: string;
  outdir: string;
}) => Promise<{ workerPath: string; innerBundlePath: string }> =
  cliEmitWorker as typeof CLI_EMIT_WORKER;

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
    // Use a POST so the wrapper bypasses the ASSETS probe and goes
    // straight to the inner bundle (the dispatch order is "ASSETS first
    // for GET/HEAD; inner first for everything else").
    const env = { ANTHROPIC_API_KEY: "sk-test-1234" };
    const request = new Request("https://worker.test/api/foo", { method: "POST" });

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

  it("serves static assets via env.ASSETS for GET requests (head-injected SSG HTML wins over dynamic SSR)", async () => {
    // Wave 10 / zudo-doc#1355 fix: GET requests probe env.ASSETS FIRST.
    // CF Pages' asset server resolves no-trailing-slash URLs to their
    // canonical /index.html form (e.g. "/docs/foo" → "/docs/foo/" via
    // 308, then to dist/docs/foo/index.html). The static HTML carries
    // build-time head-injection (<link rel="stylesheet">, <script
    // type="module" src="/assets/islands-…">) that the dynamic-SSR
    // path produced by the inner Hono router does NOT carry — so we
    // must hit ASSETS first, not the inner.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    // Inner bundle that, if reached, would 200 with a body that lacks
    // the head injection. The test asserts the wrapper does NOT reach
    // the inner when ASSETS resolves the URL.
    await writeFile(
      inputPath,
      `export default {
  async fetch() {
    return new Response("dynamic SSR (should not be visible)", { status: 200 });
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
        fetch: async (req: Request) => {
          assetsCalls += 1;
          // Simulate the SSG output for /docs/getting-started/.
          return new Response(
            '<!doctype html><html><head><link rel="stylesheet" href="/assets/styles.css"><script type="module" src="/assets/islands-abc.js"></script></head><body>static</body></html>',
            { status: 200, headers: { "content-type": "text/html; charset=utf-8" } },
          );
        },
      },
    };
    const ctx = {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
    };

    const request = new Request("https://worker.test/docs/getting-started");
    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(200);
    const body = await response.text();
    // ASSETS was hit (not the inner).
    expect(assetsCalls).toBe(1);
    // Static-SSG body, including the build-time head injection.
    expect(body).toContain("static");
    expect(body).toContain('rel="stylesheet"');
    expect(body).toContain("/assets/islands-abc.js");
    expect(body).not.toContain("dynamic SSR (should not be visible)");
  });

  it("falls through to the inner bundle when env.ASSETS returns 404 (genuinely dynamic routes)", async () => {
    // Mirror of the SSG path: when ASSETS does not resolve the URL,
    // hand it to the inner zfb worker. This is the path that handles
    // \`pages/api/*.tsx\` and other \`prerender = false\` routes.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    await writeFile(
      inputPath,
      `export default {
  async fetch(request) {
    return new Response("dynamic: " + new URL(request.url).pathname, {
      status: 200,
      headers: { "content-type": "text/plain" },
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
          return new Response("not found", { status: 404 });
        },
      },
    };
    const ctx = {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
    };
    const request = new Request("https://worker.test/api/ai-chat");
    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("dynamic: /api/ai-chat");
    expect(assetsCalls).toBe(1);
  });

  it("returns the asset response unchanged when ASSETS issues a 308 redirect (CF Pages trailing-slash canonicalisation)", async () => {
    // CF Pages' asset server returns 308 to canonicalise no-trailing-
    // slash URLs to their /index.html form. The wrapper must propagate
    // that 308 verbatim so the browser follows it to the SSG output.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    await writeFile(
      inputPath,
      `export default {
  async fetch() {
    return new Response("should not be called", { status: 200 });
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

    const env = {
      ASSETS: {
        fetch: async (req: Request) => {
          // CF Pages emits 308 with Location: <path>/.
          const url = new URL(req.url);
          return new Response(null, {
            status: 308,
            headers: { location: url.pathname + "/" },
          });
        },
      },
    };
    const ctx = {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
    };
    const request = new Request("https://worker.test/docs/getting-started");
    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(308);
    expect(response.headers.get("location")).toBe("/docs/getting-started/");
  });

  it("skips the ASSETS probe for non-GET/HEAD methods", async () => {
    // POSTs to an API route must not be probed against the read-only
    // asset server — they go straight to the inner SSR worker so the
    // page-side handler can read the request body and respond.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    await writeFile(
      inputPath,
      `export default {
  async fetch(request) {
    return new Response(JSON.stringify({ method: request.method }), {
      status: 200,
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
    const request = new Request("https://worker.test/api/ai-chat", { method: "POST" });
    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ method: "POST" });
    // ASSETS was NOT hit — POST goes straight to the inner.
    expect(assetsCalls).toBe(0);
  });

  it("works without env.ASSETS bound (e.g. the legacy single-bundle mode)", async () => {
    // Defensive: if env.ASSETS is absent (e.g. a custom deploy not
    // using CF Pages), the wrapper must still dispatch GETs to the
    // inner without crashing.
    const dir = await scratch();
    const inputPath = join(dir, "inner.mjs");
    await writeFile(
      inputPath,
      `export default {
  async fetch() {
    return new Response("inner only", { status: 200 });
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

    const env = {}; // no ASSETS binding
    const ctx = {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
    };
    const request = new Request("https://worker.test/some/path");
    const response = await worker.default.fetch(request, env, ctx);
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("inner only");
  });

  it("CLI's emitWorker (bin/cli.mjs) emits the same files as build.ts's emitWorker", async () => {
    // Exercises the CLI's emitWorker export directly so the shared
    // emit-worker.mjs refactor cannot drift silently. Both paths must
    // produce identical _worker.js and _zfb_inner.mjs content.
    const dir = await scratch();
    const inputPath = join(dir, "bundle.mjs");
    await writeFile(
      inputPath,
      `export default { fetch: async () => new Response("cli-test", { status: 200 }) };\n`,
      "utf8",
    );

    const cliDir = join(dir, "cli-dist");
    const tsDir = join(dir, "ts-dist");

    const [cliOut, tsOut] = await Promise.all([
      CLI_EMIT_WORKER({ inputBundlePath: inputPath, outdir: cliDir }),
      emitWorker({ inputBundlePath: inputPath, outdir: tsDir }),
    ]);

    const [cliWorker, tsWorker] = await Promise.all([
      readFile(cliOut.workerPath, "utf8"),
      readFile(tsOut.workerPath, "utf8"),
    ]);
    const [cliInner, tsInner] = await Promise.all([
      readFile(cliOut.innerBundlePath, "utf8"),
      readFile(tsOut.innerBundlePath, "utf8"),
    ]);

    // Both paths must write the same wrapper and the same copied bundle.
    expect(cliWorker).toBe(tsWorker);
    expect(cliInner).toBe(tsInner);

    // Returned path shapes are correct.
    expect(cliOut.workerPath).toBe(join(cliDir, "_worker.js"));
    expect(cliOut.innerBundlePath).toBe(join(cliDir, "_zfb_inner.mjs"));
  });
});
