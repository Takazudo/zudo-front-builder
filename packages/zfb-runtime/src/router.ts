// `@takazudo/zfb-runtime` — Hono-based page router.
//
// `createPageRouter` is the JS-side entry point for ADR-007's SSG-first
// architecture. The pipeline goes:
//
//   user pages/ + content/ + layouts/ + components/
//     → esbuild bundle (T3)            // single ESM file
//     → embedded V8 host (T6)          // same WinterCG surface as CF Workers
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
// at runtime against the running embedded host. The router exposes a
// synthetic internal endpoint:
//
//   GET /__paths__/<percent-encoded-route-key>
//
// When a page registered at `route` has a `paths` export, the handler calls
// it and returns the JSON-serialized array as `application/json`. If the
// `paths` export is missing or throws, the response is a descriptive 500.
// This endpoint is only meant for the build pipeline — it is safe to leave
// registered in production. During build/check/dev route scans, `zfb-router`
// rejects `pages/__paths__/<...>` with `RouterError::ReservedRoutePrefix`.
// The warning below remains as belt-and-braces for `createPageRouter` callers
// that hand-build `pages` and therefore bypass the scanner.

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
 * - `default`: the JSX page component. Called with the props returned by
 *   `getStaticProps` (if exported) or the `props` from the matching
 *   `paths()` entry (for dynamic routes). The return value is fed straight
 *   to the framework adapter's `renderToString`. A dynamic route with no
 *   `paths()` export is called with `{ params: urlParams }` instead of
 *   `{}` — see `createPageRouter`'s JSDoc for the full componentInput
 *   derivation table.
 * - `prerender`: literal `false` opts a route OUT of build-time SSG (T5
 *   contract). The page router still serves it under the embedded V8 host
 *   so dev mode behaves identically; SSG callers filter the route list before
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
 *   **Evaluated once per router instance** — the result is memoised and
 *   shared across the `/__paths__` handler and all per-page render
 *   requests. This matches the build-time-enumerator contract: every
 *   entry rendered within a single build sees the same paths() snapshot.
 *   Dev mode is safe because each file-save triggers a fresh bundle,
 *   which creates a new router instance with a clean memo.
 * - `getStaticProps`: optional async function for static routes that need
 *   to fetch data at build/render time. Called once per request (before
 *   `default`). Must return `{ props: Record<string, unknown> }`. The
 *   returned `props` are spread into the `default` component's props.
 */
export interface PageModule {
  readonly default: (props: Record<string, unknown>) => unknown;
  readonly prerender?: boolean;
  readonly contentType?: string;
  readonly headings?: readonly PageHeading[];
  readonly paths?: () => unknown[] | Promise<unknown[]>;
  readonly getStaticProps?: () => Promise<{ props: Record<string, unknown> }>;
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
  /**
   * When `true`, the 500 body for render errors includes the full JS stack
   * trace. When `false`, only the message + route are included. When
   * omitted, the runtime checks `globalThis.__zfb.ssrDebug` at request time:
   * that flag is set by the embedded V8 build/dev host (`globals_shim.js`)
   * and is absent on the production Cloudflare Workers runtime.
   *
   * Default is effectively OFF for production (no flag ⇒ message + route only).
   * Useful for explicit injection in unit tests without global mutation.
   */
  readonly includeErrorStack?: boolean;
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
 * verbatim because the embedded V8 host does NOT auto-set a charset.
 */
const DEFAULT_CONTENT_TYPE = "text/html; charset=utf-8";

/**
 * Issue #530: prepend `<!doctype html>\n` to SSR-rendered HTML bodies that
 * lack a doctype declaration, mirroring the guard in
 * `crates/zfb-build/src/renderer.rs::render_one_inner` (issue #524).
 *
 * Gate conditions (all must hold to prepend):
 *   1. `contentType` media-type (before `;`) is `text/html` (case-insensitive).
 *   2. The body, after stripping an optional UTF-8 BOM (U+FEFF) and leading
 *      ASCII whitespace, starts with `<html` (case-insensitive) — i.e. it is
 *      an `<html>`-rooted document, not XML / JSON / a fragment.
 *   3. The body (same stripped prefix) does NOT already begin with `<!doctype`
 *      (case-insensitive).
 *
 * Deliberately NOT shared with the Rust side — duplication of a 5-line guard
 * is the right call rather than introducing a new cross-language boundary.
 */
function ensureHtml5Doctype(body: string, contentType: string): string {
  const mediaType = contentType.split(";")[0]?.trim().toLowerCase();
  if (mediaType !== "text/html") return body;
  // Strip optional BOM and leading whitespace to inspect the root element.
  const stripped = body.replace(/^﻿/, "").trimStart();
  if (!stripped.toLowerCase().startsWith("<!doctype")) {
    if (stripped.toLowerCase().startsWith("<html")) {
      return `<!doctype html>\n${body}`;
    }
  }
  return body;
}

/**
 * Evaluate `pathsFn` once per `routeKey`, sharing the result across all
 * concurrent and subsequent callers via a Promise stored in `memo`.
 *
 * Rejections are evicted from the memo so that a transient paths() failure
 * does not permanently poison the cache — the next request retries.
 */
async function getOrEvalPaths(
  memo: Map<string, Promise<unknown[]>>,
  routeKey: string,
  pathsFn: () => unknown[] | Promise<unknown[]>,
): Promise<unknown[]> {
  const existing = memo.get(routeKey);
  if (existing !== undefined) {
    return existing;
  }
  const promise = Promise.resolve(pathsFn()).then(
    (v) => v as unknown[],
    (err) => {
      // Evict on rejection so the next request retries.
      memo.delete(routeKey);
      return Promise.reject(err) as Promise<unknown[]>;
    },
  );
  memo.set(routeKey, promise);
  return promise;
}

/**
 * Build a page router for the SSG-first architecture (ADR-005).
 *
 * Side effects:
 *   1. Registers `opts.contentSnapshot` with the `zfb/content` module so
 *      `getCollection(name)` resolves from memory. Idempotent across
 *      calls — the latest snapshot wins (matches the documented dev-mode
 *      live-reload contract).
 *   2. Constructs an internal Hono app and registers an **all-methods**
 *      handler (`app.all`, not `app.get`) per `pages[i].route`, so an SSR
 *      route that dispatches on `request.method` — e.g. a POST endpoint
 *      under `pages/api/` — actually reaches its handler instead of being
 *      404'd by the inner router. See the comment at the `app.all` call
 *      site below. The handler imports the page module, calls
 *      `framework.renderToString(module.default(componentInput))`, and
 *      returns the string in a `Response` with the appropriate
 *      `Content-Type`. (`componentInput` is the page's props object — see
 *      the derivation table below; it is never the incoming `Request`.)
 *
 * Per-route `componentInput` (the object passed to `module.default(...)`) is
 * derived from the route pattern and the page module's exports:
 *   - Dynamic route + `paths()` export: match the URL params against the
 *     `paths()` entries and pass `{ params, ...props }` (404 on no match).
 *   - Static route + `getStaticProps()` export: pass the returned `props`.
 *   - Dynamic route with NO `paths()` export (e.g. a per-request SSR page
 *     whose slugs can't be enumerated ahead of time): pass `{ params:
 *     urlParams }` so the component still knows which URL params it is
 *     serving, rather than being invoked with `{}`.
 *   - Anything else (static route, no `getStaticProps`): pass `{}`.
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

  // Per-router-instance memo for paths() results.
  //
  // Keyed on page.route (e.g. "/blog/:slug"). Stores the in-flight or
  // settled Promise so that concurrent requests share one evaluation.
  // Rejections are NOT cached: if paths() throws the Promise is removed
  // from the map so the next request retries rather than re-propagating
  // the same error indefinitely.
  //
  // Production SSR isolates evaluate paths() once per isolate, which
  // matches the documented build-time-enumerator contract. Dev mode is
  // safe because each re-bundle creates a fresh router instance
  // (dev.rs reload_renderer), so stale paths() results never persist
  // across a file-save cycle. Watch-time invalidation is explicitly
  // out of scope per issue #507.
  const pathsMemo = new Map<string, Promise<unknown[]>>();

  // Sanity check: a user-authored route under `/__paths__/...` has its
  // GET/HEAD responses hidden by the synthetic endpoint registered below.
  // We register `/__paths__/:routeKey{.+}` first (see directly below), and
  // Hono dispatches by registration order, so it wins those requests.
  for (const page of opts.pages) {
    if (routeShadowsPathsEndpoint(page.route)) {
      // Use console.warn so the message reaches the host's tail logs
      // without bringing down the worker. The build pipeline's
      // /__paths__ requests resolve through the synthetic handler, which
      // is registered first and wins GET/HEAD requests.
      console.warn(
        `[zfb-runtime] route "${page.route}" is hidden by the synthetic /__paths__ endpoint for GET/HEAD requests (it is registered first and wins); rename the page or use a more specific pattern`,
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

    let result: unknown[];
    try {
      result = await getOrEvalPaths(pathsMemo, routeKey, mod.paths);
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
        // Hono swallow the error into a generic body. T6's embedded V8
        // host log-tail / source-map plumbing is what eventually projects
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
      //
      // Whether the route is dynamic is derived from the ROUTE PATTERN,
      // not from the captured params: for an optional catchall
      // (`/docs/:slug{.+}?`), Hono matches the bare `/docs` with NO
      // params captured at all, so `Object.keys(c.req.param())` would
      // be empty and the old gate skipped paths() entirely — rendering
      // with `{}` instead of the entry whose `slug` is `[]`.
      const declaredParams = routeParamSpecs(page.route);
      const rawUrlParams = c.req.param() ?? {};
      const urlParams: Record<string, string> = {};
      for (const [k, v] of Object.entries(rawUrlParams)) {
        // Unmatched optional params may surface as undefined — drop them
        // so "param absent" is represented uniformly.
        if (typeof v === "string") urlParams[k] = v;
      }
      const isDynamicRoute = declaredParams.length > 0;

      let componentInput: Record<string, unknown> = {};

      if (isDynamicRoute && typeof mod.paths === "function") {
        let pathsResult: unknown;
        try {
          pathsResult = await getOrEvalPaths(pathsMemo, page.route, mod.paths);
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
        //
        // Matching iterates the params DECLARED by the route pattern so
        // the zero-segment optional-catchall case is covered: when Hono
        // matched `/docs` for `/docs/:slug{.+}?` no `slug` param exists
        // in the URL, and the matching entry is the one whose param is
        // the explicit empty array (`{ slug: [] }`).
        const match = (pathsResult as PathsEntry[]).find((entry) => {
          return declaredParams.every(({ name, optionalCatchall }) => {
            const urlVal = urlParams[name];
            const paramVal = entry.params[name];
            if (urlVal === undefined) {
              // Param absent from the URL: only valid for an optional
              // catchall, and only against the explicit `[]` entry.
              return optionalCatchall && Array.isArray(paramVal) && paramVal.length === 0;
            }
            if (Array.isArray(paramVal)) {
              // catchall: paths() emits slug as string[] but Hono
              // provides it as a "/"-joined string
              return paramVal.join("/") === urlVal;
            }
            return String(paramVal) === urlVal;
          });
        });

        if (!match) {
          // The URL params do not correspond to any paths() entry —
          // this is the dev-mode equivalent of a build-time miss.
          // Hono's `c.notFound()` returns the framework's default 404,
          // which is cleaner than fabricating an empty-props render.
          return c.notFound();
        }

        // Pass the paths() entry's props directly as component props
        // (spread to top level, matching the Astro/zfb convention).
        // Also include `params` so components can access URL params if
        // needed, but individual prop keys from `props` win on collision.
        componentInput = {
          params: match.params,
          ...(match.props ?? {}),
        };
      } else if (isDynamicRoute) {
        // Dynamic route with no `paths()` export: the natural shape for a
        // per-request SSR page whose slugs can't be enumerated ahead of
        // time. Neither branch above applies (no paths() to match against,
        // and getStaticProps only fires for static routes), so without this
        // the component would be invoked with `{}` — no params, no
        // diagnostic, and no way to know which slug it is serving. Pass the
        // URL params through directly instead.
        componentInput = { params: urlParams };
      } else if (!isDynamicRoute && typeof mod.getStaticProps === "function") {
        // Static route with `getStaticProps`: call it to fetch build-time
        // data and pass the returned props to the default component. This
        // supports the `export async function getStaticProps()` pattern
        // used by static pages that need to query content collections (e.g.
        // a homepage listing all blog posts via `getCollection("blog")`).
        let staticPropsResult: unknown;
        try {
          staticPropsResult = await mod.getStaticProps();
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err);
          return c.body(`[zfb-runtime] getStaticProps() threw for "${page.route}": ${msg}`, 500, {
            "Content-Type": "text/plain; charset=utf-8",
          });
        }
        if (
          typeof staticPropsResult !== "object" ||
          staticPropsResult === null ||
          !("props" in staticPropsResult)
        ) {
          return c.body(
            `[zfb-runtime] getStaticProps() for "${page.route}" must return { props: {...} }`,
            500,
            { "Content-Type": "text/plain; charset=utf-8" },
          );
        }
        componentInput = (staticPropsResult as { props: Record<string, unknown> }).props;
      }

      // `await` so async page modules (e.g. API routes typed as
      // `(): Promise<Response>`) resolve before we inspect the value.
      // For sync pages that return a VNode/string the await is a no-op.
      //
      // Wrapped in a try/catch so that any error thrown by the component
      // or by renderToString surfaces as a descriptive 500 rather than
      // escaping to Hono's generic error handler (which discards the real
      // message). Mirrors the getStaticProps catch above.
      try {
        const result = await mod.default(componentInput);
        // API route short-circuit: a page module that returns a Response
        // directly (e.g. `pages/api/*.tsx` handlers that use Web Fetch
        // primitives instead of returning JSX) is responsible for its own
        // status, headers, and body — return it as-is rather than running
        // it through the framework SSR path. A `return` from inside a
        // `try` does not trigger the `catch`, so this passes through
        // correctly without special-casing.
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
        return c.body(ensureHtml5Doctype(html, contentType), 200, { "Content-Type": contentType });
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        // __zfb.ssrDebug is set only by the embedded V8 build/dev host
        // (globals_shim.js), never by the production Workers runtime.
        const includeStack =
          opts.includeErrorStack ??
          (globalThis as { __zfb?: { ssrDebug?: boolean } }).__zfb?.ssrDebug === true;
        const stack = includeStack && err instanceof Error && err.stack ? `\n${err.stack}` : "";
        return c.body(`[zfb-runtime] render threw for "${page.route}": ${msg}${stack}`, 500, {
          "Content-Type": "text/plain; charset=utf-8",
        });
      }
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
 * One param declared by a Hono route pattern. `optionalCatchall` is
 * `true` for the `:name{.+}?` form (file-system `[[...name]]`), whose
 * zero-segment match captures no param at all.
 */
interface RouteParamSpec {
  readonly name: string;
  readonly optionalCatchall: boolean;
}

/**
 * Parse the params declared by a Hono route pattern (the `route` string
 * the build pipeline registers, e.g. `/blog/:slug`, `/docs/:slug{.+}`,
 * `/docs/:slug{.+}?`). Static routes return an empty list.
 *
 * Only the pattern shapes emitted by `bracket_to_hono` (zfb-build) are
 * recognised — `:name`, `:name{.+}`, `:name{.+}?` — which is the entire
 * grammar the file router produces.
 */
function routeParamSpecs(route: string): RouteParamSpec[] {
  const specs: RouteParamSpec[] = [];
  for (const seg of route.split("/")) {
    if (!seg.startsWith(":")) continue;
    const optionalCatchall = seg.endsWith("?");
    const body = optionalCatchall ? seg.slice(1, -1) : seg.slice(1);
    const braceIdx = body.indexOf("{");
    const name = braceIdx === -1 ? body : body.slice(0, braceIdx);
    if (name.length > 0) {
      specs.push({ name, optionalCatchall });
    }
  }
  return specs;
}

/**
 * Check whether a user-authored route is hidden by the synthetic
 * `/__paths__/<encoded-route-key>` endpoint.
 *
 * Returns true only when the first route segment is the literal
 * `__paths__` and at least one later segment is non-empty. Those are
 * the routes matched by the synthetic endpoint's `:routeKey{.+}` pattern.
 */
function routeShadowsPathsEndpoint(route: string): boolean {
  const segments = route.replace(/^\/+/, "").split("/");
  return segments[0] === "__paths__" && segments.slice(1).some((segment) => segment.length > 0);
}
