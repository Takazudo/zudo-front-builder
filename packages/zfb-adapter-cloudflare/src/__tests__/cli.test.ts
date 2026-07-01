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

import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";
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
}) => Promise<{ workerPath: string; innerBundlePath: string; assetsIgnorePath: string }> =
  cliEmitWorker as typeof CLI_EMIT_WORKER;

const execFileAsync = promisify(execFile);
const CLI_BIN_PATH = join(dirname(fileURLToPath(import.meta.url)), "../../bin/cli.mjs");

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

type EmittedWorker = {
  default: {
    fetch: (request: Request, env: Record<string, unknown>, ctx: unknown) => Promise<Response>;
  };
};

// Emit a `_worker.js` around a synthetic inner bundle and import it as ESM.
// Each call uses a fresh mkdtemp dir, so the emitted worker path is unique
// per test and the dynamic-import cache never collides across cases.
async function emitAndImportWorker(innerSource: string): Promise<EmittedWorker> {
  const dir = await scratch();
  const inputPath = join(dir, "inner.mjs");
  await writeFile(inputPath, innerSource, "utf8");
  const out = await emitWorker({ inputBundlePath: inputPath, outdir: join(dir, "dist") });
  return (await import(pathToFileURL(out.workerPath).href)) as EmittedWorker;
}

const NOOP_CTX = {
  waitUntil: () => undefined,
  passThroughOnException: () => undefined,
};

// A synthetic inner bundle that mimics the real zfb inner router's default
// not-found: Hono's `c.notFound()` → `text/plain` "404 Not Found". Used by
// the styled-asset-404 precedence tests below.
const INNER_PLAIN_404 = `export default {
  async fetch() {
    return new Response("404 Not Found", {
      status: 404,
      headers: { "content-type": "text/plain; charset=UTF-8" },
    });
  },
};
`;

// The styled 404.html body the asset layer serves under
// not_found_handling = "404-page" (or the Pages 404.html convention).
const STYLED_404_HTML =
  '<!doctype html><html><head><link rel="stylesheet" href="/assets/s.css"></head><body>Custom styled 404 page</body></html>';

// Model the asset server's styled-404 response: 404 + text/html + a real
// Content-Length (a static file). new Response(string) does NOT auto-set
// content-length in this runtime, so we set it explicitly to match the
// platform (verified: workerd/CF serve 404.html with Content-Length).
function styledAsset404Response(body: string | null = STYLED_404_HTML): Response {
  return new Response(body, {
    status: 404,
    headers: {
      "content-type": "text/html; charset=utf-8",
      "content-length": String(STYLED_404_HTML.length),
      "x-served-by": "asset-404-page",
    },
  });
}

describe("CLI / emitWorker", () => {
  it("WORKER_WRAPPER_SOURCE re-exported by bin/cli.mjs matches the build.ts constant", () => {
    // Both src/build.ts and bin/cli.mjs import from the canonical
    // src/worker-wrapper.mjs, so they must be the same string.
    // This guards against import-chain breakage.
    expect(MJS_WRAPPER).toBe(TS_WRAPPER);
  });

  it("emits _worker.js, _zfb_inner.mjs, and .assetsignore side-by-side", async () => {
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
    expect(out.assetsIgnorePath).toBe(join(dir, "dist", ".assetsignore"));

    const wrapperBody = await readFile(out.workerPath, "utf8");
    const innerBody = await readFile(out.innerBundlePath, "utf8");
    const assetsIgnoreBody = await readFile(out.assetsIgnorePath, "utf8");
    expect(wrapperBody).toBe(TS_WRAPPER);
    expect(innerBody).toContain("hello");
    // Byte-exact: excludes the wrapper and inner bundle from the asset
    // upload so only the Worker's module graph can reach them.
    expect(assetsIgnoreBody).toBe("_worker.js\n_zfb_inner.mjs\n");
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
    // The asset server (Workers Static Assets or Cloudflare Pages)
    // resolves no-trailing-slash URLs to their canonical /index.html
    // form (e.g. "/docs/foo" → "/docs/foo/" via a redirect, then to
    // dist/docs/foo/index.html). The static HTML carries build-time
    // head-injection (<link rel="stylesheet">, <script type="module"
    // src="/assets/islands-…">) that the dynamic-SSR path produced by
    // the inner Hono router does NOT carry — so we must hit ASSETS
    // first, not the inner.
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

  // ---------------------------------------------------------------------------
  // Styled-404 precedence (issue #1322). When the asset probe 404s AND the
  // inner worker also 404s, the wrapper prefers the earlier styled asset
  // response ONLY when the inner 404 is the framework's generic default
  // (Hono's `text/plain` "404 Not Found", or a bare 404 with no content-type)
  // — otherwise the styled 404.html is lost and users see the inner's
  // plain-text 404. An inner 404 that declares another content-type is a
  // deliberate response (a `text/html` rendered not-found page, or a
  // structured `application/json` API error) and WINS over the static styled
  // asset page.
  // ---------------------------------------------------------------------------

  it('prefers the styled asset 404 over the inner plain 404 (not_found_handling = "404-page")', async () => {
    // Asset layer returns 404 WITH the styled 404.html body; the inner router
    // returns its generic text/plain 404. The styled page must win.
    const worker = await emitAndImportWorker(INNER_PLAIN_404);

    let assetsCalls = 0;
    const env = {
      ASSETS: {
        fetch: async () => {
          assetsCalls += 1;
          return styledAsset404Response();
        },
      },
    };

    const request = new Request("https://worker.test/no-such-page-xyz");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    expect(assetsCalls).toBe(1);
    expect(response.headers.get("x-served-by")).toBe("asset-404-page");
    const body = await response.text();
    expect(body).toContain("Custom styled 404 page");
    expect(body).toContain('rel="stylesheet"');
    // The inner's plain-text 404 must NOT leak through.
    expect(body).not.toBe("404 Not Found");
  });

  it('keeps the inner 404 when the asset 404 has an empty/non-HTML body (not_found_handling = "none")', async () => {
    // "none" is Cloudflare's default 404 — no styled body. The predicate
    // (text/html + Content-Length) is false, so the inner 404 wins, exactly
    // as before this fix.
    const worker = await emitAndImportWorker(`export default {
  async fetch() {
    return new Response("inner plain 404", {
      status: 404,
      headers: { "content-type": "text/plain; charset=UTF-8" },
    });
  },
};
`);

    const env = {
      ASSETS: {
        // not_found_handling = "none": null body, no content-type/length.
        fetch: async () => new Response(null, { status: 404 }),
      },
    };

    const request = new Request("https://worker.test/no-such-page");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    expect(await response.text()).toBe("inner plain 404");
  });

  it("keeps the inner 404 for a bare Pages advanced-mode asset 404 (no styled body)", async () => {
    // Pages advanced mode without the 404.html convention: the asset 404 is a
    // plain-text "Not Found" with no HTML body. Predicate false → inner wins.
    const worker = await emitAndImportWorker(`export default {
  async fetch() {
    return new Response("inner dynamic 404", {
      status: 404,
      headers: { "content-type": "text/plain; charset=UTF-8" },
    });
  },
};
`);

    const env = {
      ASSETS: {
        fetch: async () =>
          new Response("Not Found", {
            status: 404,
            headers: { "content-type": "text/plain; charset=utf-8", "content-length": "9" },
          }),
      },
    };

    const request = new Request("https://worker.test/missing");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    expect(await response.text()).toBe("inner dynamic 404");
  });

  it("prefers the styled asset 404 for the Pages 404.html convention (styled html body)", async () => {
    // Pages advanced mode WITH the 404.html convention: the asset 404 carries
    // the styled html page, same shape as the Workers "404-page" case → asset
    // wins over the inner's generic 404.
    const worker = await emitAndImportWorker(INNER_PLAIN_404);

    const env = {
      ASSETS: {
        fetch: async () => styledAsset404Response(),
      },
    };

    const request = new Request("https://worker.test/nope");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    expect(response.headers.get("x-served-by")).toBe("asset-404-page");
    expect(await response.text()).toContain("Custom styled 404 page");
  });

  it("keeps the inner response when it is non-404, even if the asset 404 is styled (dynamic route wins)", async () => {
    // A prerender=false route that the asset layer does not resolve: the asset
    // returns a styled 404, but the inner serves the route with a 200. The
    // styled-404 preference only fires when the inner ALSO 404s.
    const worker = await emitAndImportWorker(`export default {
  async fetch(request) {
    return new Response("dynamic ok: " + new URL(request.url).pathname, {
      status: 200,
      headers: { "content-type": "text/plain" },
    });
  },
};
`);

    const env = {
      ASSETS: {
        fetch: async () => styledAsset404Response(),
      },
    };

    const request = new Request("https://worker.test/api/live-data");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(200);
    expect(await response.text()).toBe("dynamic ok: /api/live-data");
    expect(response.headers.get("x-served-by")).toBeNull();
  });

  it("does NOT stomp an intentional inner JSON API 404 with the styled asset 404", async () => {
    // An API route that deliberately 404s with a machine-readable payload
    // (application/json). Even though the asset layer offers a styled 404
    // page, the inner's structured 404 is the API contract and must survive.
    const worker = await emitAndImportWorker(`export default {
  async fetch() {
    return new Response(JSON.stringify({ error: "not found" }), {
      status: 404,
      headers: { "content-type": "application/json; charset=utf-8" },
    });
  },
};
`);

    const env = {
      ASSETS: {
        fetch: async () => styledAsset404Response(),
      },
    };

    const request = new Request("https://worker.test/api/widgets/999");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    expect(response.headers.get("content-type")).toContain("application/json");
    expect(response.headers.get("x-served-by")).toBeNull();
    expect(await response.json()).toEqual({ error: "not found" });
  });

  it("keeps an inner text/html 404 (a rendered custom not-found page) over the styled asset 404", async () => {
    // A prerender=false dynamic route (e.g. a [slug] page) that SSRs its OWN
    // 404 page: status 404, content-type text/html, a real rendered body.
    // This is a deliberate response, NOT the framework's generic text/plain
    // default, so it must WIN over the static site-wide styled asset 404 —
    // otherwise the route's bespoke not-found page would be silently replaced
    // by the generic one. (Narrowing from the original #1322 fix, which
    // classified any non-JSON inner 404 as "generic".)
    const worker = await emitAndImportWorker(`export default {
  async fetch() {
    return new Response(
      "<!doctype html><html><body><h1>No such product</h1></body></html>",
      {
        status: 404,
        headers: { "content-type": "text/html; charset=utf-8", "x-served-by": "inner" },
      },
    );
  },
};
`);

    const env = {
      ASSETS: {
        fetch: async () => styledAsset404Response(),
      },
    };

    const request = new Request("https://worker.test/products/does-not-exist");
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    // The inner's rendered page won, not the site-wide styled asset 404.
    expect(response.headers.get("x-served-by")).toBe("inner");
    const body = await response.text();
    expect(body).toContain("No such product");
    expect(body).not.toContain("Custom styled 404 page");
  });

  it("HEAD: prefers the styled asset 404 headers/status over the inner plain 404 (no body)", async () => {
    // A HEAD asset 404 carries no body but the same content-type/Content-Length
    // headers a GET would. The header-only predicate fires symmetrically, so
    // the styled asset response (status + headers) wins for HEAD too.
    const worker = await emitAndImportWorker(`export default {
  async fetch() {
    return new Response(null, {
      status: 404,
      headers: { "content-type": "text/plain; charset=UTF-8", "x-served-by": "inner" },
    });
  },
};
`);

    const env = {
      ASSETS: {
        // HEAD: null body, but content-type + Content-Length present.
        fetch: async () => styledAsset404Response(null),
      },
    };

    const request = new Request("https://worker.test/no-such-page", { method: "HEAD" });
    const response = await worker.default.fetch(request, env, NOOP_CTX);

    expect(response.status).toBe(404);
    expect(response.headers.get("content-type")).toContain("text/html");
    expect(response.headers.get("content-length")).toBe(String(STYLED_404_HTML.length));
    // The asset response won, not the inner.
    expect(response.headers.get("x-served-by")).toBe("asset-404-page");
  });

  it("returns the asset response unchanged when ASSETS issues a 308 redirect (CF Pages trailing-slash canonicalisation)", async () => {
    // The asset server returns a redirect to canonicalise no-trailing-
    // slash URLs to their /index.html form — 308 on Cloudflare Pages,
    // 307 on Workers Static Assets. The wrapper must propagate the
    // response unchanged so the browser follows it to the SSG output;
    // this test exercises the Pages 308 case (any non-404 status must
    // pass through verbatim, regardless of which platform issued it).
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
          // CF Pages emits 308 with Location: <path>/ (Workers Static
          // Assets uses 307 for the same canonicalisation).
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
    // Defensive: if env.ASSETS is absent (e.g. a custom deploy using
    // neither Workers Static Assets nor CF Pages), the wrapper must
    // still dispatch GETs to the inner without crashing.
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
    expect(cliOut.assetsIgnorePath).toBe(join(cliDir, ".assetsignore"));
  });

  it("running the CLI binary prints three `wrote <path>` lines, the last for .assetsignore", async () => {
    // Runs the real `bin/cli.mjs` as a subprocess (the shape a consumer's
    // build step actually invokes) rather than importing its exports, so
    // this asserts the stdout contract end-to-end.
    const dir = await scratch();
    const inputPath = join(dir, "bundle.mjs");
    await writeFile(
      inputPath,
      `export default { fetch: async () => new Response("cli-stdout-test", { status: 200 }) };\n`,
      "utf8",
    );
    const outdir = join(dir, "dist");

    const { stdout } = await execFileAsync("node", [
      CLI_BIN_PATH,
      "bundle",
      inputPath,
      "--outdir",
      outdir,
    ]);
    const lines = stdout.trim().split("\n");

    expect(lines).toHaveLength(3);
    expect(lines[0]).toBe(`wrote ${join(outdir, "_worker.js")}`);
    expect(lines[1]).toBe(`wrote ${join(outdir, "_zfb_inner.mjs")}`);
    expect(lines[2]).toBe(`wrote ${join(outdir, ".assetsignore")}`);

    const assetsIgnoreBody = await readFile(join(outdir, ".assetsignore"), "utf8");
    expect(assetsIgnoreBody).toBe("_worker.js\n_zfb_inner.mjs\n");
  });
});
