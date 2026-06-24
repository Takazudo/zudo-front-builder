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
 * Loader signature for a virtual-module registration. Must return the
 * **complete ESM module source text** as a string — the bundler /
 * embedded V8 host feeds the returned string in as the module's
 * source verbatim. The loader is invoked **exactly once per build**
 * (and once per `zfb dev` host boot) the first time any consumer
 * imports the registered specifier; subsequent imports of the same
 * specifier re-use the memoised result.
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
 *   // package-owned page route (prerendered at build, dev-routed in dev)
 *   injectRoute("/preset-page", "./pages/preset-page.tsx");
 *   // dev-only mock endpoint
 *   if (command === "dev") {
 *     injectRoute("/api/dev/x", "./scripts/dev-x.ts");
 *   }
 * }
 * ```
 *
 * The hook's surface is intentionally **closed**: only
 * `injectRoute`, `addVirtualModule`, `addAlias`. There is no
 * `addRemarkPlugin` / `addRehypePlugin` / `addMarkdownVisitor` — by
 * design (see the concept doc for the rationale).
 */
export type ZfbSetupContext = {
  /**
   * Active zfb command. `"build"` during `zfb build`; `"dev"` during
   * `zfb dev`. Affects `injectRoute`: in `"dev"`, `"/"` is reserved for
   * the devMiddleware catch-all and is rejected; in `"build"`, a `"/"`
   * package route is allowed (see [`injectRoute`](#injectRoute)).
   */
  command: "build" | "dev";
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
   * runs **exactly once per build** at first import.
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
   * - In **dev**, the dev server routes the matched URL into the page
   *   rendering pipeline as if `entrypoint` were a `pages/<...>.tsx`
   *   file. `"/"` is reserved for the devMiddleware catch-all and is
   *   rejected.
   * - In **build** (package-owned routes), the route is materialised
   *   into a per-build overlay pages root and **prerendered** through
   *   the normal scan → bundle → render pipeline, so a preset can own a
   *   route without the project shipping a `pages/` stub file. A `"/"`
   *   package route is allowed (it becomes the project's root page,
   *   enabling a truly empty/absent user `pages/`). A package route
   *   whose URL shape collides with a user `pages/` route is dropped
   *   (user `pages/` wins).
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
};

/**
 * The plugin-module shape. `name` is informational (the resolved module
 * specifier wins for identification on the Rust side) and helps the
 * plugin self-identify in logs.
 *
 * Four optional hooks; declaration-order matters when multiple plugins
 * touch the same surface. Each hook is independent — a plugin may
 * declare any subset:
 *
 * - `setup` (#255) — register virtual modules, aliases, injected
 *   routes. Runs once at host boot, before `preBuild`.
 * - `preBuild` — file-generation work that downstream stages will
 *   see. Runs once per `zfb build` and once per `zfb dev` boot.
 * - `postBuild` — finalisation work that runs after `dist/` has been
 *   written.
 * - `devMiddleware` — register HTTP handlers for ad-hoc dev-only
 *   URLs. Per-request dispatch, distinct from `injectRoute` (which
 *   goes through the page renderer).
 */
export type ZfbPlugin = {
  /** Plugin display name; surfaces in error / log lines. */
  name: string;
  setup?(ctx: ZfbSetupContext): Promise<void> | void;
  preBuild?(ctx: ZfbBuildHookContext): Promise<void> | void;
  postBuild?(ctx: ZfbBuildHookContext): Promise<void> | void;
  devMiddleware?(ctx: ZfbDevMiddlewareContext): Promise<void> | void;
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
