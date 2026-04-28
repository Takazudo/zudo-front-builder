// `@takazudo/zfb-runtime` — Hono-based page router.
//
// `createPageRouter` is the JS-side entry point for ADR-005's SSG-first
// architecture. The pipeline goes:
//
//   user pages/ + content/ + layouts/ + components/
//     → esbuild bundle (T3)            // single ESM file
//     → miniflare subprocess (T6)      // workerd; same runtime as CF Workers
//     → createPageRouter({ pages, contentSnapshot, framework })
//     → (request) => Promise<Response>
//
// The bundle's Worker entry point shape is documented in the package
// README. The contract is intentionally minimal: one factory call returns
// one fetch handler. Hono is an implementation detail — callers should
// not depend on the Hono types leaking through. The exposed surface is
// the four types here plus the `createPageRouter` function.
//
// Side effect on init: registers the supplied `ContentSnapshot` with
// `zfb/content`'s module-level snapshot bridge so any page module
// importing `getCollection("...")` resolves from memory rather than
// touching the Node `fs` API (the Worker runtime has no `fs`).

import { Hono } from "hono";
import { setContentSnapshot } from "zfb/content";

import type { FrameworkAdapter } from "./framework.js";
import type { ContentSnapshot } from "./snapshot.js";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * Heading metadata emitted by the MDX `headings` export (T4). Cross-ref:
 * `crates/zfb-content/src/mdx_jsx_emit.rs`. Optional on the page-module
 * shape — non-MDX pages won't carry it.
 */
export interface PageHeading {
  readonly depth: number;
  readonly slug: string;
  readonly text: string;
}

/**
 * The shape every page module must export.
 *
 * - `default`: the JSX page component. Called with no props (today —
 *   wave 2 will extend this for `paths()` dynamic routes). The return
 *   value is fed straight to the framework adapter's `renderToString`.
 * - `prerender`: literal `false` opts a route OUT of build-time SSG (T5
 *   contract). The page router still serves it under miniflare so dev
 *   mode behaves identically; SSG callers filter the route list before
 *   driving the renderer.
 * - `content_type`: optional override for non-HTML routes (e.g.
 *   `application/xml` for `rss.xml.tsx`). Default is
 *   `text/html; charset=utf-8`. Cross-ref shipped #49.
 * - `headings`: optional list emitted by MDX (T4).
 */
export interface PageModule {
  readonly default: (props: Record<string, unknown>) => unknown;
  readonly prerender?: boolean;
  readonly content_type?: string;
  readonly headings?: readonly PageHeading[];
}

/**
 * One page registered with the router.
 *
 * `route` is a Hono path pattern (e.g. `/`, `/blog/:slug`,
 * `/blog/page/:page`). `module` is a thunk so the bundle can use code
 * splitting if it wants to — today the bundler emits everything as one
 * ESM file and the thunk simply returns the already-loaded module.
 */
export interface PageDefinition {
  readonly route: string;
  readonly module: () => Promise<PageModule>;
}

/** Options accepted by [`createPageRouter`]. */
export interface CreatePageRouterOptions {
  /** Pages to register. Order does not affect routing — Hono dispatches by path. */
  readonly pages: readonly PageDefinition[];
  /**
   * In-memory content snapshot. Embedded into the bundle by T3; the
   * router hands it to `zfb/content` so user pages reading content via
   * `getCollection(...)` resolve synchronously from memory.
   */
  readonly contentSnapshot: ContentSnapshot;
  /** Framework adapter pinning the SSR call. */
  readonly framework: FrameworkAdapter;
}

/**
 * Fetch-handler shape returned by [`createPageRouter`]. Shaped as a plain
 * function (not a Hono `app`) so the consumer's contract is exactly
 * "Worker-style fetch handler" with no leaked framework types.
 */
export type PageRouter = (request: Request) => Promise<Response>;

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

/**
 * Default content-type when a page module does not override.
 *
 * Aligned with #49's per-page `content_type` convention: the default
 * served for HTML pages is `text/html; charset=utf-8`. Tests pin this
 * verbatim because miniflare/workerd does NOT auto-set a charset.
 */
const DEFAULT_CONTENT_TYPE = "text/html; charset=utf-8";

/**
 * Build a page router for the SSG-first architecture (ADR-005).
 *
 * Side effects:
 *   1. Registers `opts.contentSnapshot` with the `zfb/content` module so
 *      `getCollection(name)` resolves from memory. Idempotent across
 *      calls — the latest snapshot wins (matches the documented dev-mode
 *      live-reload contract).
 *   2. Constructs an internal Hono app and registers a GET handler per
 *      `pages[i].route`. The handler imports the page module, calls
 *      `framework.renderToString(module.default({}))`, and returns the
 *      string in a `Response` with the appropriate `Content-Type`.
 *
 * The returned function is a plain `(request) => Promise<Response>` so a
 * Worker entry point can `export default { fetch: createPageRouter(...) }`
 * directly.
 */
export function createPageRouter(opts: CreatePageRouterOptions): PageRouter {
  setContentSnapshot(opts.contentSnapshot);

  const app = new Hono();

  for (const page of opts.pages) {
    app.get(page.route, async (c) => {
      const mod = await page.module();
      if (typeof mod.default !== "function") {
        // Surface as a 500 with a well-known message rather than letting
        // Hono swallow the error into a generic body. T6's miniflare
        // log-tail / source-map plumbing is what eventually projects
        // page-side errors back to the user's TSX line; until then the
        // pinned message is the contract this layer ships.
        return c.body(
          `[zfb-runtime] page module for "${page.route}" did not export a default component`,
          500,
          { "Content-Type": "text/plain; charset=utf-8" },
        );
      }
      const vnode = mod.default({});
      const html = opts.framework.renderToString(vnode);
      const contentType = mod.content_type ?? DEFAULT_CONTENT_TYPE;
      return c.body(html, 200, { "Content-Type": contentType });
    });
  }

  // Hono's `app.fetch` returns `Response | Promise<Response>`. The
  // public router contract is unconditionally async; using an `async`
  // wrapper (rather than `Promise.resolve(...)`) ensures any
  // synchronous throw inside `app.fetch` is converted to a rejected
  // promise instead of escaping the caller's `await`.
  return async (request) => await app.fetch(request);
}
