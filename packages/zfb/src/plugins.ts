// `zfb/plugins` — TypeScript helper for the zfb plugin lifecycle.
//
// A plugin is a JS module whose default export is a [`ZfbPlugin`] object.
// `zfb.config.ts` references plugins by `name` (npm bare specifier or a
// `./`-relative path); the zfb config loader resolves each `name` to an
// absolute module specifier and the Rust-side plugin host loads the
// module via dynamic `import()` and dispatches the lifecycle hooks.
//
// Sub 3 / issue #108 — initial drop. Three optional hooks: `preBuild`,
// `postBuild`, `devMiddleware`. Astro-migration epic #253 / sub-issue
// #255 adds a fourth: `setup`, which runs once before `preBuild` and
// lets plugins register virtual modules, import aliases, and dev-only
// injected routes. None of the hooks see real Node IPC objects across
// the boundary; everything is JSON-friendly.
//
// ## Inline functions are NOT supported
//
// `PluginConfig` (in `./config.ts`) carries only data. A user cannot
// inline a function in `zfb.config.ts` — the config goes through a
// JSON round-trip and any function value would be silently dropped.
// Plugins must live in their own module (npm package or local file)
// and be referenced by `name`.

/**
 * Logger handed to every plugin hook. The Rust side wraps `tracing` so
 * the same lines show up alongside the rest of the build's structured
 * logs. Hooks should prefer this over `console.log`.
 */
export type ZfbPluginLogger = {
  info(msg: string): void;
  warn(msg: string): void;
  error(msg: string): void;
};

/**
 * One emitted route in the `postBuild` route manifest (#262).
 * Present on `ctx.routes.routes` so a `postBuild` plugin can iterate
 * every URL the build produced (e.g. to write a `sitemap.xml`).
 */
export type ZfbRouteEntry = {
  /** Emitted URL path, e.g. `/`, `/blog/hello/`, `/sitemap.xml`. */
  url: string;
  /** Path under `outDir`, e.g. `index.html`, `blog/hello/index.html`, `sitemap.xml`. */
  output: string;
  /** File extension: `html`, `xml`, `rss`, `txt`, `json`, … */
  extension: string;
  /** Source page module relative to the project root, e.g. `pages/blog/[slug].tsx`. */
  source: string;
  /**
   * `true` when the page is prerendered to disk (default / SSG); `false`
   * when the page exports `prerender = false` and is served by the
   * runtime adapter (SSR — no on-disk artifact under `outDir`).
   *
   * Indexes that enumerate on-disk URLs (sitemap.xml, search-index.json,
   * etc.) should filter `r.prerender !== false` to avoid surfacing SSR
   * routes that have no static output.
   */
  prerender: boolean;
  /**
   * Bound route parameters. Absent for static routes.
   * Dynamic (`[slug]`) params are string scalars; catchall (`[...rest]`)
   * params are string arrays.
   */
  params?: Record<string, string | string[]>;
};

/**
 * The route manifest exposed on `ctx.routes` during a `postBuild` callback
 * (#262). Sorted by `url` for byte-stable output across runs.
 */
export type ZfbRouteManifest = {
  routes: ZfbRouteEntry[];
};

/**
 * Context passed to `preBuild` and `postBuild`. `outDir` is the
 * resolved absolute path of the configured `outDir` (default
 * `<projectRoot>/dist`). `projectRoot` is the directory containing
 * `zfb.config.ts`.
 *
 * `routes` is **only present on `postBuild`** calls; it is `undefined`
 * on `preBuild`. This is intentional: the route manifest is not
 * available until the build finishes writing `dist/` (#262).
 */
export type ZfbBuildHookContext = {
  /** Project root — the directory containing `zfb.config.ts`. */
  projectRoot: string;
  /** Resolved absolute path of the build output directory. */
  outDir: string;
  /** The full loaded `ZfbConfig` (data-only view). */
  config: import("./config.js").ZfbConfig;
  /** Plugin-specific options block, copied verbatim from the matching `PluginConfig.options`. */
  options: Record<string, unknown>;
  /** Logger that wraps the Rust-side `tracing` subscriber. */
  logger: ZfbPluginLogger;
  /**
   * All routes emitted by this build, sorted by URL (#262).
   * Present only on `postBuild` calls; `undefined` on `preBuild`.
   */
  routes?: ZfbRouteManifest;
};

/**
 * A request handed to a `devMiddleware` handler. Subset of the Node
 * `http.IncomingMessage` surface intentionally — the dev server is
 * Rust-side `axum`, not Node, so we expose only what survives a JSON
 * envelope hop.
 */
export type ZfbDevMiddlewareRequest = {
  method: string;
  url: string;
  /** Lower-cased header names → first value. */
  headers: Record<string, string>;
  /** Raw request body; absent for GET/HEAD. UTF-8 only — binary is out of scope for v1 dev plugins. */
  body?: string;
};

/**
 * Response returned by a `devMiddleware` handler. All fields optional
 * except `status`. `body` may be a string (UTF-8) or a base64-encoded
 * binary payload (set `bodyEncoding` to `"base64"` in that case).
 */
export type ZfbDevMiddlewareResponse = {
  status: number;
  headers?: Record<string, string>;
  body?: string;
  bodyEncoding?: "utf8" | "base64";
};

/**
 * Handler signature for a `devMiddleware` registration. The `next` callback
 * is reserved for future composition; v1 plugins should produce a response
 * directly. Returning `undefined` from the handler signals "I did not handle
 * this request" — the dev server then falls through to its built-in routes
 * (the page cache, /__zfb/livereload.js, etc.).
 */
export type ZfbDevMiddlewareHandler = (
  req: ZfbDevMiddlewareRequest,
) => Promise<ZfbDevMiddlewareResponse | undefined> | ZfbDevMiddlewareResponse | undefined;

/**
 * Context passed to `devMiddleware`. The `register` callback installs
 * one handler per URL path prefix. `path` is matched as an exact prefix
 * — a registration on `/doc-history` matches `/doc-history` and
 * `/doc-history/foo`, but NOT `/doc-historyx`.
 */
export type ZfbDevMiddlewareContext = {
  projectRoot: string;
  config: import("./config.js").ZfbConfig;
  options: Record<string, unknown>;
  logger: ZfbPluginLogger;
  /** Register an HTTP handler at `path`. Calling twice on the same path overwrites. */
  register(path: string, handler: ZfbDevMiddlewareHandler): void;
};

/**
 * Handler signature for a `previewMiddleware` registration (#1542).
 * Deliberately reuses [`ZfbDevMiddlewareRequest`] /
 * [`ZfbDevMiddlewareResponse`] verbatim — the wire shape crossing the
 * Rust↔JS boundary is genuinely the SAME for dev and preview (mirrors
 * the Rust side, which shares `DevRequest`/`DevResponse` between both
 * hooks too), so there is nothing preview-specific to say about the
 * request/response contract itself. `next` is likewise reserved for
 * future composition; returning `undefined` signals "I did not handle
 * this request" and the preview server falls through to its built-in
 * routes (static-file serving, or the wrangler-backed adapter in
 * adapter mode).
 */
export type ZfbPreviewMiddlewareHandler = (
  req: ZfbDevMiddlewareRequest,
) => Promise<ZfbDevMiddlewareResponse | undefined> | ZfbDevMiddlewareResponse | undefined;

/**
 * Context passed to `previewMiddleware` (#1542). Structurally identical
 * to [`ZfbDevMiddlewareContext`] today — one handler per URL path
 * prefix, matched the same way — but declared as its own named type
 * (unlike the request/response types above, which are reused verbatim)
 * because the *context* is where a hook-specific capability would land
 * first if one were ever added (e.g. something preview-only that
 * `devMiddleware` has no equivalent for). Keeping it a separate
 * declaration costs nothing today and avoids a breaking rename later.
 */
export type ZfbPreviewMiddlewareContext = {
  projectRoot: string;
  config: import("./config.js").ZfbConfig;
  options: Record<string, unknown>;
  logger: ZfbPluginLogger;
  /** Register an HTTP handler at `path`. Calling twice on the same path overwrites. */
  register(path: string, handler: ZfbPreviewMiddlewareHandler): void;
};

/**
 * Loader signature for a virtual-module registration. Must return the
 * **complete ESM module source text** as a string — the bundler /
 * embedded V8 host feeds the returned string in as the module's
 * source verbatim. The loader runs **eagerly**, not lazily on first
 * import: exactly once per `zfb build` run and once per `zfb dev`
 * host boot, during the setup phase right after every plugin's
 * `setup` hook has returned — even if the registered specifier is
 * never imported by any page or module. The resulting source is
 * memoised; every subsequent import of that specifier reuses it.
 * (Under `zfb preview`, `addVirtualModule` registrations are accepted
 * but inert — see [`ZfbSetupContext.command`](#command) — so the
 * loader never runs there.)
 *
 * Example:
 *
 * ```ts
 * addVirtualModule("virtual:my-data", () =>
 *   `export default ${JSON.stringify(myJson)}`,
 * );
 * ```
 */
export type ZfbVirtualModuleLoader = () => string | Promise<string>;

/**
 * Context passed to the new `setup` hook (#255). Runs once per host
 * boot, in `Config.plugins` declaration order, **before** `preBuild`.
 *
 * `ctx.command` tells the plugin which lifecycle is active so it can
 * gate per-lifecycle registrations. A dev-only mock route stays gated
 * to `"dev"`; a package-owned page route is registered unconditionally
 * (it is prerendered during a build and dev-routed during dev):
 *
 * ```ts
 * setup({ command, injectRoute }) {
 *   // package-owned page route (rendered in build and dev)
 *   injectRoute("/preset-page", "./pages/preset-page.tsx");
 *   // dev-only mock endpoint
 *   if (command === "dev") {
 *     injectRoute("/api/dev/x", "./scripts/dev-x.ts");
 *   }
 * }
 * ```
 *
 * The hook's surface is intentionally **closed**: only `injectRoute`,
 * `addVirtualModule`, `addAlias`, and `addClientEntry`. There is no
 * `addRemarkPlugin` / `addRehypePlugin` / `addMarkdownVisitor` — by
 * design (see the concept doc for the rationale).
 */
export type ZfbSetupContext = {
  /**
   * Active zfb command. `"build"` during `zfb build`; `"dev"` during
   * `zfb dev`; `"preview"` during `zfb preview` (#1542). It can guide
   * lifecycle-specific plugin behavior. `injectRoute` registrations are
   * accepted in both `"dev"` and `"build"`; user `pages/` routes retain
   * precedence over matching injected routes (see
   * [`injectRoute`](#injectRoute)).
   *
   * Under `"preview"`, `setup` still fires (Rust-side via the minimal
   * non-V8 `run_preview_setup` path) so plugin-side state
   * initialisation runs, but `zfb preview` serves an ALREADY-BUILT
   * `dist/` verbatim and never re-enters the scan → bundle → render
   * pipeline. Consequently `injectRoute` / `addVirtualModule` /
   * `addAlias` / `addClientEntry` calls made under `"preview"` are
   * accepted (for shape-consistency with `"build"`/`"dev"`) but are
   * **inert** — nothing downstream ever reads them. Only the hook's
   * side effects and a subsequent `previewMiddleware` registration do
   * anything meaningful under `"preview"`.
   */
  command: "build" | "dev" | "preview";
  /** Project root — the directory containing `zfb.config.ts`. */
  projectRoot: string;
  /** The full loaded `ZfbConfig` (data-only view). */
  config: import("./config.js").ZfbConfig;
  /** Plugin-specific options block, copied verbatim from `PluginConfig.options`. */
  options: Record<string, unknown>;
  /** Logger that wraps the Rust-side `tracing` subscriber. */
  logger: ZfbPluginLogger;

  /**
   * Register an import alias. **Exact-match-only in v1**:
   * `addAlias("@/foo", "./src/foo.tsx")` rewrites `import "@/foo"`
   * but does NOT match `import "@/foo/bar"`. Prefix-matching is
   * explicitly deferred to v2 — switch to one bare alias per file
   * until then.
   *
   * `to` is resolved relative to the project root. Two plugins
   * registering the same `from` with different `to` raises
   * `AliasConflict` and aborts the build.
   */
  addAlias(from: string, to: string): void;

  /**
   * Register a virtual module. `specifier` is a bare import
   * specifier (recommended `virtual:` prefix, not enforced).
   * `loader` returns the complete ESM source text as a string and
   * runs **eagerly, once per build/dev-boot during setup** — not
   * lazily at first import (see [`ZfbVirtualModuleLoader`]).
   *
   * Two plugins registering the same `specifier` raises
   * `VirtualModuleConflict` and aborts the build.
   */
  addVirtualModule(specifier: string, loader: ZfbVirtualModuleLoader): void;

  /**
   * Register a synthetic / package-owned page route. `pattern` uses the
   * same grammar as `pages/` filenames (`/blog/[slug]`, `/api/dev/x`,
   * `/docs/[...rest]`).
   *
   * - In **build** (package-owned routes), the route is materialised
   *   into a per-build overlay pages root and **prerendered** through
   *   the normal scan → bundle → render pipeline, so a preset can own a
   *   route without the project shipping a `pages/` stub file. A `"/"`
   *   package route becomes the project's root page when no user
   *   `pages/index` exists, enabling a truly empty/absent user `pages/`.
   *   A package route whose URL shape collides with a user `pages/` route
   *   is dropped (user `pages/` wins). This is the supported, complete path.
   * - In **dev**, both static and dynamic injected routes are rendered
   *   by `zfb dev`. Static routes (where the URL equals the pattern,
   *   e.g. `/preset-about`) are seeded into the dev route universe at
   *   boot; dynamic routes (e.g. `/preset-docs/[slug]`) are rendered
   *   on first request via a request-time synthetic entry — params are
   *   extracted from the URL by the Hono router inside the live bundle.
   *   User `pages/` files take precedence over any injected route of
   *   the same shape, including `pages/index` over an injected `"/"`.
   *   Without a user index, an injected root is staged, seeded, and served
   *   like any other static injected route. **HMR:** content the
   *   route reads from watched collections live-refreshes normally.
   *   Editing the package's **compiled entrypoint under `node_modules`**
   *   is NOT watched and requires a `zfb dev` restart (restart-only
   *   contract — a published package is not project source). **Per-route
   *   data:** an injected route loads per-route data via a **dynamic
   *   route's `paths()` export** (which returns `{ params, props }`);
   *   `getStaticProps` on a package page is not forwarded by the overlay
   *   (only `default` + the `prerender` hint are forwarded — same as
   *   `zfb build`). A route that needs per-route data should be a
   *   dynamic route whose `paths()` reads the data.
   *
   * `opts.prerender` controls the route's prerender shape during a
   * build: omit it (or `true`) for the SSG default; `false` marks an
   * SSR-shaped route, which `output: 'static'` rejects. It is build-only
   * metadata and ignored in dev.
   *
   * Two plugins registering the same `pattern` (or one plugin
   * re-registering it with a different entrypoint) raises
   * `InjectRouteConflict`.
   */
  injectRoute(pattern: string, entrypoint: string, opts?: { prerender?: boolean }): void;

  /**
   * Register a package-owned client-side side-effect entry (#1196).
   *
   * `entrypoint` **must** point to a `*.client.{ts,tsx,js,jsx}` file —
   * this is enforced (#1191 review [9]): a path missing the `.client.`
   * infix, or a bare `.client.ts` with an empty stem, throws an error
   * (`addClientEntry` JS-host validation + Rust `InvalidClientEntry`)
   * rather than being silently accepted under an invented name. The entry
   * name is derived from the filename stem minus `.client`
   * (e.g. `my-lib.client.ts` → `my-lib`), via the same canonical helper
   * as user-authored `*.client.*` discovery.
   *
   * The entry is bundled and shipped as
   * `/assets/client/<name>.js` (stable URL) / `/assets/client/<name>-<hash>.js`
   * (production, hashed). User-authored files win on name collision —
   * the registered entry is silently dropped when a user-authored file of
   * the same name exists in the discovery roots.
   *
   * Two plugins registering the same entry name with different entrypoints
   * raises `ClientEntryConflict` and aborts the build.
   *
   * `entrypoint` is resolved relative to the project root if given as a
   * relative path (same rule as `injectRoute`).
   */
  addClientEntry(entrypoint: string): void;
};

/**
 * The plugin-module shape. `name` is informational (the resolved module
 * specifier wins for identification on the Rust side) and helps the
 * plugin self-identify in logs.
 *
 * Five optional hooks; declaration-order matters when multiple plugins
 * touch the same surface. Each hook is independent — a plugin may
 * declare any subset:
 *
 * - `setup` (#255) — register virtual modules, aliases, injected
 *   routes. Runs once at host boot, before `preBuild`. Also runs under
 *   `zfb preview` (#1542) via the minimal non-V8 `run_preview_setup`
 *   path — see [`ZfbSetupContext.command`](#command) for what is and
 *   isn't meaningful there.
 * - `preBuild` — file-generation work that downstream stages will
 *   see. Runs once per `zfb build` and once per `zfb dev` boot. Does
 *   **NOT** fire under `zfb preview` (#1542) — preview serves an
 *   already-built `dist/` and never re-triggers file generation.
 * - `postBuild` — finalisation work that runs after `dist/` has been
 *   written. Does not fire under `zfb preview` either, for the same
 *   reason as `preBuild`.
 * - `devMiddleware` — register HTTP handlers for ad-hoc dev-only
 *   URLs. Per-request dispatch, distinct from `injectRoute` (which
 *   goes through the page renderer). Fires only during `zfb dev`.
 * - `previewMiddleware` (#1542) — register HTTP handlers for ad-hoc
 *   preview-only URLs. Same register-context shape as `devMiddleware`,
 *   fires only during `zfb preview`. A plugin wanting coverage in both
 *   modes registers the same handler under both hooks — `zfb` does
 *   NOT reuse a `devMiddleware` registration for preview automatically
 *   (explicit per-mode opt-in, by design).
 */
export type ZfbPlugin = {
  /** Plugin display name; surfaces in error / log lines. */
  name: string;
  setup?(ctx: ZfbSetupContext): Promise<void> | void;
  preBuild?(ctx: ZfbBuildHookContext): Promise<void> | void;
  postBuild?(ctx: ZfbBuildHookContext): Promise<void> | void;
  devMiddleware?(ctx: ZfbDevMiddlewareContext): Promise<void> | void;
  previewMiddleware?(ctx: ZfbPreviewMiddlewareContext): Promise<void> | void;
};

/**
 * Identity helper that types the supplied object as a [`ZfbPlugin`].
 * Use as the default export of a plugin module so editors surface
 * field-level types and typos surface at compile time.
 *
 * ```ts
 * import { definePlugin } from "@takazudo/zfb/plugins";
 *
 * export default definePlugin({
 *   name: "my-plugin",
 *   async preBuild({ outDir, logger }) {
 *     logger.info(`generating index into ${outDir}`);
 *   },
 * });
 * ```
 */
export function definePlugin(plugin: ZfbPlugin): ZfbPlugin {
  return plugin;
}
