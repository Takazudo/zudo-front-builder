// `@takazudo/zfb-adapter-cloudflare` — Cloudflare Workers Static Assets
// adapter for the zfb framework (also deployable to Cloudflare Pages
// advanced mode).
//
// This entry (`./`) is the **Workers-runtime** surface. It is safe to
// bundle into a Cloudflare Worker and does not depend on any Node-only
// built-ins beyond `node:async_hooks` (which workerd polyfills).
//
// Usage in a `prerender = false` route:
//
//   import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";
//
//   export const prerender = false;
//   export default async function ApiRoute() {
//     const { env, ctx } = getCloudflareContext<{ ANTHROPIC_API_KEY: string }>();
//     // env.ANTHROPIC_API_KEY, ctx.waitUntil(...), etc.
//   }
//
// For Node-only build helpers (e.g. `emitWorker`), import the `./build`
// sub-entry instead:
//
//   import { emitWorker } from "@takazudo/zfb-adapter-cloudflare/build";
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
 * let consumers describe a compatible richer shape with the second generic
 * on [`getCloudflareContext`] when they need additional execution-context
 * bindings. That type is supplied by the caller; it is not detected or
 * validated at runtime.
 */
export interface CloudflareExecutionContext {
  /** Extends the lifetime of the request beyond the response. */
  waitUntil(promise: Promise<unknown>): void;
  /** Falls through to the static origin on uncaught exceptions. */
  passThroughOnException(): void;
}

/**
 * Per-request Cloudflare context. `Env` defaults to `unknown` and `Ctx` to
 * the minimal execution-context shape. Consumers can supply narrower or
 * richer types at the call site when they know the runtime shape.
 */
export interface CloudflareContext<
  Env = unknown,
  Ctx extends CloudflareExecutionContext = CloudflareExecutionContext,
> {
  /** CF env bindings (secrets, KV, D1, …) wired up via wrangler.toml. */
  readonly env: Env;
  /** ExecutionContext for waitUntil / passThroughOnException and any declared extensions. */
  readonly ctx: Ctx;
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
export function runWithCloudflareContext<T, C extends CloudflareContext = CloudflareContext>(
  context: C,
  fn: () => T,
): T {
  return getStorage().run(context, fn);
}

/**
 * Read the current Cloudflare context. Throws if called outside a
 * Cloudflare request scope (e.g. from a build-time SSG render). Catch
 * the error and gate on `prerender = false` if you need a route to work
 * in both modes.
 *
 * `Env` describes the bindings available to the caller, while `Ctx` can
 * describe a compatible richer execution context. These generic arguments
 * are caller assertions: this function does not inspect runtime bindings or
 * feature-detect optional properties. Guard optional fields before use.
 */
export function getCloudflareContext<
  Env = unknown,
  Ctx extends CloudflareExecutionContext = CloudflareExecutionContext,
>(): CloudflareContext<Env, Ctx> {
  const c = getStorage().getStore();
  if (!c) {
    throw new Error(
      "[zfb-adapter-cloudflare] getCloudflareContext() called outside a Cloudflare request scope. " +
        "This usually means the route was rendered at build time (SSG) instead of dispatched by " +
        "the Worker. Add `export const prerender = false;` to the page if it needs Cloudflare bindings.",
    );
  }
  return c as CloudflareContext<Env, Ctx>;
}
