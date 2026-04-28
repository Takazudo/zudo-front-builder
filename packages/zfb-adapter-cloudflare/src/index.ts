// `@takazudo/zfb-adapter-cloudflare` — Cloudflare Pages adapter for the
// zfb framework.
//
// This module ships two surfaces:
//
//   1. A **runtime** API user code calls from inside a `prerender = false`
//      route to read the request-scoped Cloudflare bindings:
//
//        import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";
//
//        export const prerender = false;
//        export default async function ApiRoute() {
//          const { env, ctx } = getCloudflareContext<{ ANTHROPIC_API_KEY: string }>();
//          // env.ANTHROPIC_API_KEY, ctx.waitUntil(...), etc.
//        }
//
//   2. A **build-time** helper [`emitWorker`] that produces the
//      `dist/_worker.js` Cloudflare Pages advanced mode expects. The CLI
//      wrapper at `bin/cli.mjs` exposes this through
//      `zfb-adapter-cloudflare bundle <input> --outdir <dir>`.
//
// ## Why AsyncLocalStorage on a globalThis registry
//
// Cloudflare Workers can process multiple requests concurrently in the
// same isolate. A naïve `globalThis.__env = env` write would race across
// requests. AsyncLocalStorage gives us a per-request scope that survives
// `await` points without interfering with sibling requests.
//
// We register the storage instance on `globalThis` under a stable key
// (`__zfb_cf_adapter_als__`) so the wrapper at `_worker.js` and the user
// pages bundled together can share the same instance even when the
// adapter module ends up duplicated in the final bundle graph (e.g. when
// the wrapper file is emitted side-by-side with the inner bundle and
// each pulls in its own copy of this module). Module-instance identity
// is the property AsyncLocalStorage relies on; the registry pattern is
// what makes it survive bundler duplication.

import { AsyncLocalStorage } from "node:async_hooks";

/**
 * Cloudflare execution context — minimal projection of the workerd
 * `ExecutionContext` interface. We do not depend on `@cloudflare/workers-types`
 * at the type level here because that would force every consumer of this
 * package to install it; instead we keep a minimal structural shape and
 * let users widen it via the generic on [`getCloudflareContext`] when
 * they need richer bindings.
 */
export interface CloudflareExecutionContext {
  /** Extends the lifetime of the request beyond the response. */
  waitUntil(promise: Promise<unknown>): void;
  /** Falls through to the static origin on uncaught exceptions. */
  passThroughOnException(): void;
}

/**
 * Per-request Cloudflare context. `Env` defaults to `unknown` so the
 * caller can narrow it at the call site (recommended) or leave it open.
 */
export interface CloudflareContext<Env = unknown> {
  /** CF env bindings (secrets, KV, D1, …) wired up via wrangler.toml. */
  readonly env: Env;
  /** ExecutionContext for waitUntil / passThroughOnException. */
  readonly ctx: CloudflareExecutionContext;
  /** The original Request, useful when handlers want headers / URL. */
  readonly request: Request;
}

/** Stable globalThis key the registry pattern uses. */
const STORAGE_KEY = "__zfb_cf_adapter_als__";

interface RegistryGlobal {
  [STORAGE_KEY]?: AsyncLocalStorage<CloudflareContext>;
}

/**
 * Acquire (or lazily create) the singleton AsyncLocalStorage instance.
 *
 * Stored on `globalThis` under a stable key so the wrapper at
 * `_worker.js` and the user bundle share state even if the adapter
 * module ends up duplicated in the final module graph.
 */
function getStorage(): AsyncLocalStorage<CloudflareContext> {
  const g = globalThis as unknown as RegistryGlobal;
  let als = g[STORAGE_KEY];
  if (!als) {
    als = new AsyncLocalStorage<CloudflareContext>();
    g[STORAGE_KEY] = als;
  }
  return als;
}

/**
 * Establish a Cloudflare context for the duration of `fn`. Used by the
 * `_worker.js` wrapper; not normally called by user code.
 */
export function runWithCloudflareContext<T>(context: CloudflareContext, fn: () => T): T {
  return getStorage().run(context, fn);
}

/**
 * Read the current Cloudflare context. Throws if called outside a
 * Cloudflare request scope (e.g. from a build-time SSG render). Catch
 * the error and gate on `prerender = false` if you need a route to work
 * in both modes.
 *
 * The `Env` generic narrows the bindings shape — passing it is
 * recommended so TypeScript catches typos like `env.ANTRHOPIC_KEY`.
 */
export function getCloudflareContext<Env = unknown>(): CloudflareContext<Env> {
  const c = getStorage().getStore();
  if (!c) {
    throw new Error(
      "[zfb-adapter-cloudflare] getCloudflareContext() called outside a Cloudflare request scope. " +
        "This usually means the route was rendered at build time (SSG) instead of dispatched by " +
        "the Worker. Add `export const prerender = false;` to the page if it needs Cloudflare bindings.",
    );
  }
  return c as CloudflareContext<Env>;
}

// ---------------------------------------------------------------------------
// Build-time CLI helper
// ---------------------------------------------------------------------------

/**
 * Inputs to [`emitWorker`].
 */
export interface EmitWorkerInput {
  /**
   * Absolute path to the input ESM bundle produced by `zfb-build`. The
   * bundle must export a Workers-shaped `default { fetch: (request) =>
   * Promise<Response> }` (this is the contract `zfb_build::bundler`
   * pins). The file is copied verbatim next to the emitted wrapper so
   * relative imports inside it keep resolving.
   */
  readonly inputBundlePath: string;
  /**
   * Absolute path to the output directory. The emitter creates it if
   * missing and writes `_worker.js` plus `_zfb_inner.mjs` (the copied
   * input bundle) into it.
   */
  readonly outdir: string;
}

/**
 * Output paths the emitter produced. Returned for callers that want to
 * log them (the Rust orchestrator surfaces them in build output).
 */
export interface EmitWorkerOutput {
  readonly workerPath: string;
  readonly innerBundlePath: string;
}

/**
 * Emit a Cloudflare Pages `_worker.js` that wraps the zfb input bundle.
 *
 * Output shape (two files in `outdir`):
 *
 *   _worker.js       — entry imported by Cloudflare Pages advanced mode
 *   _zfb_inner.mjs   — the input bundle, copied verbatim
 *
 * The wrapper imports the inner bundle via the relative path
 * `./_zfb_inner.mjs`. Workerd's Module loader resolves relative ESM
 * imports inside an advanced-mode `_worker.js` directory, so this layout
 * works without re-bundling.
 *
 * Why two files instead of one: re-bundling here would require a second
 * esbuild pass and would force the adapter to ship its own esbuild
 * binary slot. The two-file layout keeps the adapter dependency-free at
 * runtime — it is just `node:fs` glue.
 */
export async function emitWorker(input: EmitWorkerInput): Promise<EmitWorkerOutput> {
  const { mkdir, copyFile, writeFile } = await import("node:fs/promises");
  const { resolve, join } = await import("node:path");

  const outdir = resolve(input.outdir);
  const inputBundle = resolve(input.inputBundlePath);

  await mkdir(outdir, { recursive: true });
  const innerBundlePath = join(outdir, "_zfb_inner.mjs");
  await copyFile(inputBundle, innerBundlePath);

  const workerPath = join(outdir, "_worker.js");
  await writeFile(workerPath, WORKER_WRAPPER_SOURCE, "utf8");

  return { workerPath, innerBundlePath };
}

/**
 * The wrapper source written to `_worker.js`. Inlined as a string so the
 * emitter has no template-runtime dependency.
 *
 * The wrapper is intentionally framework-agnostic: it does not know about
 * Hono, the page router, or the page list. It only knows that the inner
 * bundle exports a `default { fetch }` and that the wrapper must thread
 * env/ctx through AsyncLocalStorage so user code reading
 * `getCloudflareContext()` sees the right values.
 *
 * The AsyncLocalStorage is acquired through the same globalThis registry
 * pattern this module uses. Even if the wrapper's module instance and
 * the user bundle's module instance are different (they are — they live
 * in different ESM graphs), the registry guarantees they share the same
 * AsyncLocalStorage and therefore the same per-request store.
 */
export const WORKER_WRAPPER_SOURCE = `// AUTO-GENERATED by @takazudo/zfb-adapter-cloudflare. Do not edit.
//
// Cloudflare Pages advanced mode entry. Forwards (request, env, ctx) to
// the inner zfb worker bundle, exposing env/ctx to user code via
// AsyncLocalStorage under a stable globalThis key.
//
// The same key is read by @takazudo/zfb-adapter-cloudflare's
// getCloudflareContext() inside the user bundle, so the two ends share
// state even though they live in separate ESM module instances.
import { AsyncLocalStorage } from "node:async_hooks";
import inner from "./_zfb_inner.mjs";

const STORAGE_KEY = "__zfb_cf_adapter_als__";

function getStorage() {
  const g = globalThis;
  let als = g[STORAGE_KEY];
  if (!als) {
    als = new AsyncLocalStorage();
    g[STORAGE_KEY] = als;
  }
  return als;
}

export default {
  fetch(request, env, ctx) {
    const store = { env, ctx, request };
    return getStorage().run(store, () => inner.fetch(request));
  },
};
`;
