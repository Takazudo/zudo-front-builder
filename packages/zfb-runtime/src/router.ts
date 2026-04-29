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
//
// ## Synthetic `__paths__` endpoint
//
// The Rust build pipeline needs to evaluate non-literal `paths()` exports
// (e.g. those that `await import("@takazudo/zfb/content")` and call `getCollection`)
// at runtime against the running worker. To avoid a second miniflare
// subprocess, the router exposes a synthetic internal endpoint:
//
//   GET /__paths__/<percent-encoded-route-key>
//
// When a page registered at `route` has a `paths` export, the handler calls
// it and returns the JSON-serialized array as `application/json`. If the
// `paths` export is missing or throws, the response is a descriptive 500.
// This endpoint is only meant for the build pipeline — it is safe to leave
// registered in production because no user-authored route should start with
// `/__paths__/` (the build pipeline rejects any route that conflicts).

import { Hono } from "hono";
import { setContentSnapshot } from "@takazudo/zfb/content";

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
 * - `contentType`: optional override for non-HTML routes (e.g.
 *   `application/xml` for `rss.xml.tsx`). Default is
 *   `text/html; charset=utf-8`. Cross-ref shipped #49.
 * - `headings`: optional list emitted by MDX (T4).
 * - `paths`: optional dynamic-route enumerator. Called at build time by
 *   the `__paths__` synthetic endpoint to produce the concrete URL list
 *   for this route template. May be async. Returns an array of
 *   `{ params, props? }` objects identical in shape to the Astro/zfb
 *   `paths()` contract.
 */
export interface PageModule {
  readonly default: (props: Record<string, unknown>) => unknown;
  readonly prerender?: boolean;
  readonly contentType?: string;
  readonly headings?: readonly PageHeading[];
  readonly paths?: () => unknown[] | Promise<unknown[]>;
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
 * Aligned with #49's per-page `contentType` convention: the default
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

  // Build a lookup map: Hono route pattern → PageDefinition, for the
  // `__paths__` synthetic endpoint below. The map is keyed on the `route`
  // string exactly as the caller supplied it (e.g. "/blog/:slug").
  const pagesByRoute = new Map<string, PageDefinition>();
  for (const page of opts.pages) {
    pagesByRoute.set(page.route, page);
  }

  // Sanity check: a user-authored route that happens to start with
  // `/__paths__` (or a top-level catchall like `/:slug{.+}`) would
  // shadow the synthetic endpoint registered below if Hono dispatched
  // by registration order. We register `/__paths__/:routeKey{.+}` first
  // (see directly below) but still warn on collisions so users do not
  // ship a page that conflicts with the build pipeline's wire format.
  for (const page of opts.pages) {
    if (routeShadowsPathsEndpoint(page.route)) {
      // Use console.warn so the message reaches miniflare's tail logs
      // without bringing down the worker. The build pipeline's
      // /__paths__ requests still resolve correctly because the
      // synthetic handler is registered first.
      // eslint-disable-next-line no-console
      console.warn(
        `[zfb-runtime] route "${page.route}" may shadow the synthetic /__paths__ endpoint; rename the page or use a more specific pattern`,
      );
    }
  }

  // -------------------------------------------------------------------------
  // Synthetic `/__paths__/<encoded-route-key>` endpoint.
  //
  // Called by the Rust build pipeline (crates/zfb/src/render_pipeline.rs)
  // to evaluate non-literal `paths()` exports at runtime. The route key is
  // the Hono pattern for the page (e.g. `/blog/:slug`) percent-encoded so
  // it survives in the URL path segment. The response is a JSON array of
  // `{ params, props? }` objects identical to the `paths()` contract.
  //
  // Pattern: `/__paths__/:routeKey{.+}` — the `{.+}` quantifier (Hono's
  // regex-segment syntax) allows slashes inside the route key so
  // `/blog/:slug` decodes correctly from `/__paths__/%2Fblog%2F%3Aslug`.
  //
  // IMPORTANT: this handler is registered BEFORE user routes so a
  // user-authored top-level catchall (e.g. `/:wildcard{.+}`) cannot
  // shadow it — Hono dispatches in registration order.
  // -------------------------------------------------------------------------
  app.get("/__paths__/:routeKey{.+}", async (c) => {
    // Hono's `c.req.param("routeKey")` already URL-decodes the captured
    // segment when it contains a `%`, so a single decode is correct
    // here — no explicit `decodeURIComponent` (that would be a
    // double-decode and break literal `%` characters in route keys).
    const routeKey = c.req.param("routeKey");

    const page = pagesByRoute.get(routeKey);
    if (!page) {
      return c.body(
        `[zfb-runtime] /__paths__: no page registered for route key "${routeKey}"`,
        404,
        { "Content-Type": "text/plain; charset=utf-8" },
      );
    }

    const mod = await page.module();
    if (typeof mod.paths !== "function") {
      return c.body(
        `[zfb-runtime] /__paths__: page module for "${routeKey}" has no paths() export`,
        404,
        { "Content-Type": "text/plain; charset=utf-8" },
      );
    }

    let result: unknown;
    try {
      result = await mod.paths();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      return c.body(`[zfb-runtime] /__paths__: paths() threw for "${routeKey}": ${msg}`, 500, {
        "Content-Type": "text/plain; charset=utf-8",
      });
    }

    return c.body(JSON.stringify(result), 200, {
      "Content-Type": "application/json; charset=utf-8",
    });
  });

  for (const page of opts.pages) {
    // `app.all` (vs `app.get`) so SSR routes whose page handler dispatches
    // by `request.method` (e.g. POST API endpoints like
    // `pages/api/*.tsx`) actually reach the handler. The handler is then
    // responsible for returning a method-appropriate status (e.g. 405 for
    // an unsupported verb). With `app.get` the inner router would 404
    // before the handler ever ran, leaving e.g. `POST /api/foo` looking
    // identical to a missing route.
    app.all(page.route, async (c) => {
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

      // For dynamic routes that export `paths()`, look up the concrete
      // entry for this URL by matching the URL params against the
      // paths() results. This implements the ADR-002 contract:
      //   paths() → [{ params, props? }]
      //   render(url) → find matching entry → pass { params, props } to default()
      //
      // For static routes (no `paths()` export, no URL params), we pass
      // an empty object — the component signature has no required props.
      // For dynamic routes whose URL params do not match any paths()
      // entry, we return a 404 rather than rendering with empty props.
      const rawUrlParams = c.req.param();
      const urlParams = (rawUrlParams ?? {}) as Record<string, string>;
      const hasDynamicParams = Object.keys(urlParams).length > 0;

      let componentInput: Record<string, unknown> = {};

      if (hasDynamicParams && typeof mod.paths === "function") {
        let pathsResult: unknown;
        try {
          pathsResult = await mod.paths();
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err);
          return c.body(`[zfb-runtime] paths() threw for "${page.route}": ${msg}`, 500, {
            "Content-Type": "text/plain; charset=utf-8",
          });
        }

        if (!Array.isArray(pathsResult)) {
          return c.body(`[zfb-runtime] paths() for "${page.route}" did not return an array`, 500, {
            "Content-Type": "text/plain; charset=utf-8",
          });
        }

        // Validate each entry's shape per-entry rather than using a bare
        // cast — matches the strictness the Rust pipeline applies on the
        // same wire format. Any malformed entry surfaces as a 500.
        for (const entry of pathsResult) {
          if (!isPathsEntry(entry)) {
            return c.body(
              `[zfb-runtime] paths() for "${page.route}" returned an entry without a valid params object`,
              500,
              { "Content-Type": "text/plain; charset=utf-8" },
            );
          }
        }

        // Find the entry whose params match the URL params for this
        // request. For catchall params (e.g. slug for
        // /docs/[...slug]), Hono returns a slash-joined string (e.g.
        // "guides/install"), so we compare against the paths() entry's
        // params.slug joined with "/".
        const match = (pathsResult as PathsEntry[]).find((entry) => {
          return Object.entries(urlParams).every(([k, v]) => {
            const paramVal = entry.params[k];
            if (Array.isArray(paramVal)) {
              // catchall: paths() emits slug as string[] but Hono
              // provides it as a "/"-joined string
              return paramVal.join("/") === v;
            }
            return String(paramVal) === v;
          });
        });

        if (!match) {
          // The URL params do not correspond to any paths() entry —
          // this is the dev-mode equivalent of a build-time miss.
          // Hono's `c.notFound()` returns the framework's default 404,
          // which is cleaner than fabricating an empty-props render.
          return c.notFound();
        }

        componentInput = {
          params: match.params,
          props: match.props ?? {},
        };
      }

      // `await` so async page modules (e.g. API routes typed as
      // `(): Promise<Response>`) resolve before we inspect the value.
      // For sync pages that return a VNode/string the await is a no-op.
      const result = await mod.default(componentInput);
      // API route short-circuit: a page module that returns a Response
      // directly (e.g. `pages/api/*.tsx` handlers that use Web Fetch
      // primitives instead of returning JSX) is responsible for its own
      // status, headers, and body — return it as-is rather than running
      // it through the framework SSR path.
      if (result instanceof Response) {
        return result;
      }
      // Non-HTML routes (e.g. `sitemap.xml.tsx`, `feed.xml.tsx`) commonly
      // return their body as a pre-serialised `string` instead of a
      // VNode. Routing those through `framework.renderToString` would
      // HTML-escape the angle brackets and ampersands, producing
      // garbage XML. Pass strings through verbatim; only wrap actual
      // VNodes.
      const html = typeof result === "string" ? result : opts.framework.renderToString(result);
      const contentType = mod.contentType ?? DEFAULT_CONTENT_TYPE;
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

/**
 * Shape of one entry produced by a page's `paths()` export. The
 * `params` object is required (every entry needs to identify the URL
 * it represents); `props` is optional (the page may render purely from
 * the URL params).
 */
interface PathsEntry {
  readonly params: Record<string, unknown>;
  readonly props?: Record<string, unknown>;
}

/**
 * Type guard for one `paths()` entry. Mirrors the strictness of the
 * Rust pipeline's `paths()` resolver — every entry must be a non-null
 * object whose `params` is itself a non-null object.
 */
function isPathsEntry(x: unknown): x is PathsEntry {
  if (typeof x !== "object" || x === null) return false;
  const params = (x as { params?: unknown }).params;
  return typeof params === "object" && params !== null;
}

/**
 * Heuristic check for user-authored routes that may shadow the
 * synthetic `/__paths__/<encoded-route-key>` endpoint.
 *
 * Returns true when the route literal contains `/__paths__` (an
 * obvious collision) or when its first segment is a Hono catchall
 * (`:name{.+}`) or a file-system catchall (`[...name]`) at the root —
 * those would match `/__paths__/...` once Hono dispatches to them.
 */
function routeShadowsPathsEndpoint(route: string): boolean {
  if (route.includes("/__paths__")) {
    return true;
  }
  // Strip the leading slash and look at the first segment only —
  // anything deeper cannot match `/__paths__` because the literal
  // first segment differs.
  const firstSeg = route.replace(/^\/+/, "").split("/")[0] ?? "";
  // Hono regex-quantifier catchall: `:name{.+}`.
  if (/^:[A-Za-z_][\w]*\{\.[+*]\}$/.test(firstSeg)) {
    return true;
  }
  // File-system catchall (pre-bracket-to-hono): `[...name]`.
  if (/^\[\.\.\.[A-Za-z_][\w]*\]$/.test(firstSeg)) {
    return true;
  }
  return false;
}
