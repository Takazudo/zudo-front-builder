// D1 binding acceptance test for #311 (S2 — SSR + Cloudflare D1).
//
// Goal: prove an SSR route (`prerender = false`) built with this
// adapter can read a Cloudflare Worker D1 binding (`env.DB`) from
// inside its handler.
//
// ## Why a synthetic D1 stub instead of a live wrangler dev / miniflare run
//
// The wrapper at `worker-wrapper.mjs` stores the whole `env` object
// verbatim in AsyncLocalStorage and never inspects its members. A D1
// binding is "just another object on `env`" — `env.DB` is opaque to
// the wrapper, exactly like `env.ANTHROPIC_API_KEY` in `cli.test.ts`.
//
// Therefore the architectural claim "an SSR route can reach `env.DB`"
// is fully exercised by threading a `D1Database`-shaped stub through
// the produced `_worker.js` via the same `emitWorker` + dynamic-import
// pattern the existing acceptance test uses. A real miniflare D1
// would test miniflare's SQLite, not this adapter's threading. The
// stub below implements the public `D1Database` surface
// (`prepare → bind → first/all/run`) so the page-side code reads
// exactly as it would against a real binding.
//
// The `__inbox/s2-ssr-d1-findings.md` doc records the live
// `wrangler dev` / `wrangler d1` lifecycle a real deploy uses.

import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import { emitWorker } from "../build.js";

let scratchDirs: string[] = [];

afterEach(async () => {
  for (const d of scratchDirs) {
    await rm(d, { recursive: true, force: true });
  }
  scratchDirs = [];
});

async function scratch(): Promise<string> {
  const d = await mkdtemp(join(tmpdir(), "zfb-adapter-cf-d1-"));
  scratchDirs.push(d);
  return d;
}

// ---------------------------------------------------------------------------
// Minimal in-memory D1Database stub.
//
// Mirrors the public Cloudflare `D1Database` surface used by SSR
// handlers: `prepare(sql)` → a statement that supports `.bind(...)`,
// `.first()`, `.all()`, `.run()`. The query engine is a deliberately
// tiny dispatcher keyed on a SQL substring — enough to exercise the
// adapter's binding-passthrough without pulling in a SQL parser.
// ---------------------------------------------------------------------------

interface ProductRow {
  id: number;
  name: string;
  price_cents: number;
}

function makeD1Stub(rows: ProductRow[]) {
  function makeStatement(sql: string, params: unknown[] = []) {
    return {
      bind(...args: unknown[]) {
        return makeStatement(sql, args);
      },
      async all() {
        if (/from products/i.test(sql)) {
          return { results: rows, success: true, meta: {} };
        }
        return { results: [], success: true, meta: {} };
      },
      async first() {
        if (/where id = \?/i.test(sql)) {
          const id = params[0];
          return rows.find((r) => r.id === id) ?? null;
        }
        return rows[0] ?? null;
      },
      async run() {
        return { success: true, meta: { changes: 1 } };
      },
    };
  }
  return {
    prepare(sql: string) {
      return makeStatement(sql);
    },
  };
}

describe("D1 binding (env.DB) reaches an SSR route handler", () => {
  it("an SSR handler can query env.DB.prepare(...).all() and see rows", async () => {
    // The inner bundle stands in for a `prerender = false` page that
    // imports `getCloudflareContext()` from this adapter. We inline
    // the read-side (the globalThis-keyed AsyncLocalStorage) so the
    // test does not need to bundle the user package — identical to
    // the env-passthrough test in `cli.test.ts`.
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
    // This is exactly the shape a webshop SSR route would write.
    const { results } = await env.DB.prepare("SELECT * FROM products").all();
    return new Response(JSON.stringify({ products: results }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  },
};
`,
      "utf8",
    );

    const out = await emitWorker({ inputBundlePath: inputPath, outdir: join(dir, "dist") });
    const worker = (await import(pathToFileURL(out.workerPath).href)) as {
      default: {
        fetch: (request: Request, env: Record<string, unknown>, ctx: unknown) => Promise<Response>;
      };
    };

    const rows: ProductRow[] = [
      { id: 1, name: "Hoodie", price_cents: 4200 },
      { id: 2, name: "Mug", price_cents: 1200 },
    ];
    const env = { DB: makeD1Stub(rows) };
    const ctx = { waitUntil: () => undefined, passThroughOnException: () => undefined };

    // POST so the wrapper bypasses the ASSETS probe and goes straight
    // to the inner SSR worker — matching how a dynamic API route is hit.
    const request = new Request("https://shop.test/api/products", { method: "POST" });
    const response = await worker.default.fetch(request, env, ctx);

    expect(response.status).toBe(200);
    const body = (await response.json()) as { products: ProductRow[] };
    expect(body.products).toEqual(rows);
  });

  it("an SSR handler can read a single row via env.DB.prepare(...).bind(id).first()", async () => {
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
    const env = getStore()?.env ?? {};
    const id = Number(new URL(request.url).searchParams.get("id"));
    const row = await env.DB.prepare("SELECT * FROM products WHERE id = ?").bind(id).first();
    return new Response(JSON.stringify({ row }), {
      status: row ? 200 : 404,
      headers: { "content-type": "application/json" },
    });
  },
};
`,
      "utf8",
    );

    const out = await emitWorker({ inputBundlePath: inputPath, outdir: join(dir, "dist") });
    const worker = (await import(pathToFileURL(out.workerPath).href)) as {
      default: {
        fetch: (request: Request, env: Record<string, unknown>, ctx: unknown) => Promise<Response>;
      };
    };

    const rows: ProductRow[] = [
      { id: 1, name: "Hoodie", price_cents: 4200 },
      { id: 2, name: "Mug", price_cents: 1200 },
    ];
    const env = { DB: makeD1Stub(rows) };
    const ctx = { waitUntil: () => undefined, passThroughOnException: () => undefined };

    const response = await worker.default.fetch(
      new Request("https://shop.test/api/product?id=2", { method: "POST" }),
      env,
      ctx,
    );
    expect(response.status).toBe(200);
    const body = (await response.json()) as { row: ProductRow };
    expect(body.row).toEqual({ id: 2, name: "Mug", price_cents: 1200 });
  });

  it("getCloudflareContext<Env>() narrows env to a D1-typed binding", async () => {
    // This proves the *typed* path the adapter publishes: a route
    // narrows `env` with a generic that includes `DB: D1Database`.
    // We import the real `getCloudflareContext` and drive it inside a
    // `runWithCloudflareContext` scope (the wrapper does this in prod).
    const { getCloudflareContext, runWithCloudflareContext } = await import("../index.js");

    interface ShopEnv {
      DB: { prepare(sql: string): { all(): Promise<{ results: ProductRow[] }> } };
    }

    const rows: ProductRow[] = [{ id: 7, name: "Cap", price_cents: 1900 }];
    const ctx = { waitUntil: () => undefined, passThroughOnException: () => undefined };
    const request = new Request("https://shop.test/api/products", { method: "POST" });

    const observed = await runWithCloudflareContext(
      { env: { DB: makeD1Stub(rows) }, ctx, request },
      async () => {
        const { env } = getCloudflareContext<ShopEnv>();
        const { results } = await env.DB.prepare("SELECT * FROM products").all();
        return results;
      },
    );

    expect(observed).toEqual(rows);
  });
});
