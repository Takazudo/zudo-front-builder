// `zfb/config` — TypeScript helper for the `zfb.config.ts` form.
//
// The zfb config loader (`crates/zfb/src/config.rs`) accepts both
// `zfb.config.ts` and `zfb.config.json`; TS wins when both files are
// present. JSON remains accepted for projects predating the TS loader,
// while new projects should prefer the TS form for editor types and
// `defineConfig` autocomplete.
//
// At parse time, zfb bundles the user's `zfb.config.ts` with esbuild and
// aliases this `zfb/config` import to an internal stub that re-exports
// `defineConfig` as the identity function — so a user project does not
// need the `zfb` npm package installed locally just to be parsed.
//
// The shape mirrors the Rust `Config` struct one-for-one. Keep them in
// sync; the `defineConfig` identity helper is the single anchor point.

export type Framework = "preact" | "react";

export type CollectionDef = {
  /** Identifier used at the call site (e.g. `"blog"`). */
  name: string;
  /** Directory (relative to the project root) holding the entries. */
  path: string;
  /** Optional schema. Enforced by `zfb check`. */
  schema?: Record<string, unknown>;
  /**
   * Optional include globs (Astro-style, evaluated relative to `path`).
   * When set and non-empty, an entry is kept only if at least one
   * pattern matches its relative path. When omitted or empty, no
   * include-filtering happens. Patterns use the `globset` dialect
   * (Unix-style: `*`, `**`, `?`, `[…]`).
   */
  include?: string[];
  /**
   * Optional exclude globs. When set, an entry is dropped if any
   * pattern matches its relative path. Evaluated AFTER `include`.
   * Together they mirror Astro's `['**\/*.mdx', '!**\/*.en.mdx']`
   * convention (zfb splits the negative side into its own field).
   */
  exclude?: string[];
  /**
   * Optional suffix to strip from each kept entry's slug + module
   * specifier. Use with multi-locale layouts where one source
   * directory holds both `foo.mdx` (default locale) and `foo.en.mdx`
   * (locale override) — set `idStripSuffix: ".en"` so the EN
   * collection's slugs round-trip as `foo` instead of `foo.en`.
   */
  idStripSuffix?: string;
  /**
   * Opt-in to a `path` that escapes the project root via `..` (e.g. a
   * monorepo-shared content dir living outside this package). Default
   * `false` keeps the standard project-root guard. Absolute paths and
   * Windows drive-relative/prefix forms are rejected regardless of
   * this flag — only `..`-relative escapes are relaxed.
   *
   * Security note: if this collection comes from a preset, the preset
   * author — not the consuming project — controls `path`. Setting
   * `allowOutsideRoot: true` on a preset-provided collection widens
   * the project's read surface to wherever that preset points, so
   * treat it the same as any other preset-granted filesystem access.
   */
  allowOutsideRoot?: boolean;
};

export type TailwindConfig = {
  /** Whether Tailwind is enabled. Default: `true`. */
  enabled?: boolean;
};

/**
 * Prefetch options. Mirrors `PrefetchConfig` in `crates/zfb/src/config.rs`.
 */
export type PrefetchConfig = {
  /**
   * Disable prefetch entirely.
   *
   * When `true`, the bundler emits `globalThis.__zfb.prefetchDisabled = true`
   * in `entry.mjs`, and `<ClientRouter />` renders
   * `<meta name="zfb-prefetch-disabled" content="true">` in `<head>`.
   * The sibling prefetch-core module reads that meta tag at `init()` time
   * and short-circuits — no prefetch wiring runs.
   *
   * The flag is site-wide and static — set once at bundle-emit time,
   * never recomputed per-page. Default: `false`.
   */
  disabled?: boolean;
};

/**
 * Bundler options. Mirrors `BundleConfig` in `crates/zfb/src/config.rs`.
 */
export type BundleConfig = {
  /**
   * Project-relative glob patterns (gitignore-style) for source files
   * the bundler must NOT pull into the esbuild graph.
   *
   * Why this exists: an eager `import.meta.glob('components/**\/*.stories.tsx',
   * { eager: true })` expands to a static import of every matched file. If a
   * matched file imports a CJS-only package whose `package.json` resolves only
   * via `main`/`module` or a `require`-only `exports` condition (e.g.
   * `msw` → `path-to-regexp@6`), esbuild — invoked with `--platform=neutral`
   * for the worker bundle — rejects it with "Could not resolve … Main fields
   * must be configured explicitly when using the neutral platform." Listing the
   * offending file here keeps the migration build green.
   *
   * Each pattern is matched against the file's path RELATIVE TO THE PROJECT
   * ROOT, in POSIX form (e.g. `components/Foo.stories.tsx` or
   * `components/**\/*.stories.tsx`). A matched file is:
   *
   * - never copied/symlinked into the bundler's shadow tree, and
   * - dropped from any eager `import.meta.glob(...)` expansion that would
   *   otherwise statically import it.
   *
   * Unset / empty → behaviour is byte-identical to a build without this knob:
   * no files are skipped.
   *
   * Mirrors `Config::bundle` in crates/zfb/src/config.rs.
   */
  exclude?: string[];

  /**
   * Explicit esbuild `main-fields` list for the `--platform=neutral` page/SSR
   * pass. Under `neutral` esbuild's main-fields list is EMPTY by default, so a
   * dep resolved purely via `package.json` `main`/`module` (no `exports` map)
   * is rejected ("The "main" field here was ignored. Main fields must be
   * configured explicitly when using the neutral platform."). Set e.g.
   * `["main", "module"]` to let such CJS-main-only deps resolve (#676 —
   * `msw` → `path-to-regexp@6`). Applies to every framework; unset/empty →
   * byte-identical to a build without the knob (the React-only `main,module`
   * shim still applies).
   *
   * Mirrors `BundleConfig::main_fields` in `crates/zfb/src/config.rs`.
   */
  mainFields?: string[];

  /**
   * Bare specifiers to mark external in the `--platform=neutral` page/SSR
   * pass, so esbuild leaves them unbundled instead of resolving them (the
   * other #676 escape hatch — externalize a CJS-only dep rather than
   * resolving it). Appended to the framework-provided externals. Unset/empty
   * → no extra externals.
   *
   * Mirrors `BundleConfig::external` in `crates/zfb/src/config.rs`.
   */
  external?: string[];

  /**
   * Additional esbuild loaders keyed by file extension (for example
   * `{ ".txt": "text" }`). Only inline loaders are supported: `file` and
   * `copy` are intentionally excluded because they emit sibling assets the
   * client bundlers do not publish. `.css`, `.module.css`, `.mdx`, and `.md`
   * are reserved by zfb and rejected during config validation.
   */
  loaders?: Record<string, "text" | "json" | "base64" | "dataurl" | "binary" | "empty">;

  /**
   * Operator-authored esbuild define substitutions. Values are raw esbuild
   * expressions; string values must be pre-quoted JSON (for example
   * `{ __APP_NAME__: '"my-app"' }`). The mode-owned keys
   * `import.meta.env.PROD`, `import.meta.env.DEV`, and
   * `process.env.NODE_ENV` are reserved and rejected at config-load time.
   */
  define?: Record<string, string>;
};

/**
 * One plugin entry in `zfb.config.ts`.
 *
 * `name` MUST be a module reference that Node's resolver can locate from
 * the project root. The zfb config loader
 * (`crates/zfb-config-loader/js/config-loader.mjs`) resolves it to an
 * absolute module specifier and the build / dev plugin host loads it via
 * dynamic `import()`:
 *
 * - `"./plugins/my-plugin.mjs"` / `"../shared/plugin.mjs"` —
 *   path-relative to the project root (the dir containing `zfb.config.ts`).
 * - `"/abs/path/to/plugin.mjs"` — absolute filesystem path.
 * - `"@takazudo/zfb-plugin-search"` / `"my-plugin"` — npm bare specifier
 *   resolved against the project's `node_modules`.
 *
 * Inline-function hooks are NOT supported; the plugin module's default
 * export must be a [`ZfbPlugin`] (see `@takazudo/zfb/plugins`).
 *
 * `options` is passed verbatim to the plugin's hook contexts; treat
 * the schema as plugin-specific.
 */
export type PluginConfig = {
  name: string;
  options?: Record<string, unknown>;
};

export type ZfbConfig = {
  /** Output directory for built assets. Default: `dist`. */
  outDir?: string;
  /** Public/static directory copied verbatim. Default: `public`. */
  publicDir?: string;
  /** Optional dev/preview server bind host. */
  host?: string;
  /** Optional dev/preview server port. */
  port?: number;
  /**
   * Host header values the dev/preview server accepts when bound to a
   * non-localhost interface (`--host 0.0.0.0`, the bare `--host` LAN
   * shortcut, or `host` above) — the DNS-rebinding guard, mirroring
   * Vite's `server.allowedHosts`.
   *
   * Defaults: only consulted for non-loopback binds — the default
   * `localhost` bind skips validation entirely. `localhost`, the
   * explicitly bound host, and any IP-literal Host — `127.0.0.1`,
   * `[::1]`, the LAN URLs the startup banner prints — are always
   * allowed (DNS rebinding needs a DNS name, so raw IPs are safe;
   * Vite parity); requests with any other Host get a 403.
   *
   * Matching rules (the request Host's port is stripped first and
   * comparison is case-insensitive):
   *
   * - `"example.com"` — matches exactly that host.
   * - `".example.com"` (leading dot) — matches `example.com` and every
   *   subdomain (`api.example.com`).
   * - IPv6 entries may be written with or without brackets
   *   (`"[::1]"` / `"::1"`).
   *
   * Mirrors `Config::allowed_hosts` in `crates/zfb/src/config.rs`.
   */
  allowedHosts?: string[];
  /** JSX framework runtime. Default: `preact`. */
  framework?: Framework;
  /** Content collections. Mirrors the JSON form one-for-one. */
  collections?: CollectionDef[];
  /** Tailwind options; absent = defaults. */
  tailwind?: TailwindConfig;
  /**
   * Prefetch options. When `disabled: true`, the build emits a meta tag
   * that the runtime's prefetch-core module reads at init time to skip
   * all prefetch wiring. Mirrors `Config::prefetch` in
   * `crates/zfb/src/config.rs`.
   */
  prefetch?: PrefetchConfig;
  /**
   * Minify production HTML output from `zfb build`. Default: `false`.
   *
   * The implementation is Rust-only and does not spawn a Node.js minifier
   * subprocess. The first version is intentionally conservative: rendered
   * `.html` pages are candidates, source `.html` passthrough pages remain
   * verbatim, and non-HTML outputs are skipped.
   *
   * Mirrors `Config::minify_html` in `crates/zfb/src/config.rs`.
   */
  minifyHtml?: boolean;
  /**
   * Raise broken-link diagnostics to errors during `zfb build`, failing the
   * build (exit non-zero) instead of merely warning. Default: `false`.
   *
   * This is the effective boolean the CLI's `--strict-broken` /
   * `--no-strict-broken` tri-state resolves against. Precedence: explicit
   * CLI flag > this config field > default `false`.
   *
   * Force-enable semantics: if `markdown.features.linkValidation` is absent
   * entirely, enabling this force-enables link validation with its
   * defaults — a strict flag that silently did nothing on a bare project
   * would be a footgun.
   *
   * Scope: the `linkValidation` mechanism only. The separate
   * `resolveMarkdownLinks.onBrokenLinks` mechanism keeps its own knob and is
   * not affected by this field.
   *
   * Build-only: it does not affect `zfb dev`.
   *
   * Mirrors `Config::strict_broken_links` in `crates/zfb/src/config.rs`.
   */
  strictBrokenLinks?: boolean;
  /**
   * Fail `zfb build` (exit non-zero) when a content-collection `.md`/`.mdx`
   * entry falls back to `<pre data-zfb-content-fallback>` because its
   * compiled JSX does not parse. Default: `false`.
   *
   * This is the effective boolean the CLI's `--strict-content-bridge` /
   * `--no-strict-content-bridge` tri-state resolves against. Precedence:
   * explicit CLI flag > this config field > default `false`.
   *
   * Unlike `strictBrokenLinks`, there is no adjacent feature to
   * force-enable: the content-bridge gate always runs for every compiled
   * collection entry.
   *
   * Build-only: it does not affect `zfb dev` — dev keeps warning and
   * serving the fallback shape.
   *
   * Mirrors `Config::strict_content_bridge` in `crates/zfb/src/config.rs`.
   */
  strictContentBridge?: boolean;
  /**
   * Bundler options. `bundle.exclude` lists project-relative globs of
   * source files to keep out of the esbuild graph (e.g.
   * `["components/*.stories.tsx"]`) — see {@link BundleConfig.exclude} for
   * why this is needed. Unset → byte-identical to a build without the knob.
   * Mirrors `Config::bundle` in `crates/zfb/src/config.rs`.
   */
  bundle?: BundleConfig;
  /** User-supplied plugins. */
  plugins?: PluginConfig[];
  /**
   * Deploy-target adapter package name. Omit (or `"none"`) for a pure
   * static build — any route exporting `prerender = false` is then a
   * hard build error. A package name like
   * `"@takazudo/zfb-adapter-cloudflare"` selects the matching adapter,
   * and `zfb build` invokes that package's bin to wrap the SSR bundle
   * into a deploy-ready entry (e.g. `dist/_worker.js` for Cloudflare
   * Workers Static Assets, Pages-compatible).
   *
   * Mirrors `Config::adapter` in crates/zfb/src/config.rs.
   */
  adapter?: string;
  /**
   * Strip `.md` / `.mdx` from internal `<a href>` paths during MDX
   * compilation, and append a trailing `/` so the resulting URL shape
   * converges with the rest of the site (mirrors the JS engine's
   * `rehypeStripMdExtension`). Default: `false`.
   *
   * Enable this when content authors hand-write `[label](other.md)`
   * style references that should resolve to the rendered route URL
   * (e.g. `other/`) instead of a literal file path. Built dist and
   * `pnpm dev` honour the same flag, so previews match shipped output.
   *
   * Mirrors `Config::strip_md_ext` in crates/zfb/src/config.rs.
   */
  stripMdExt?: boolean;

  /**
   * Public URL prefix mounted in front of every absolute HTML asset
   * URL the build emits — `<link rel="stylesheet">`, `<script type="module">`,
   * and any other `/assets/...`-prefixed reference rewritten by the
   * production asset pipeline.
   *
   * Use this when the site is deployed under a sub-path (e.g.
   * `https://example.com/pj/zudo-doc/`) instead of the domain root.
   * With `base: "/pj/zudo-doc/"` the dist HTML emits
   * `<link rel="stylesheet" href="/pj/zudo-doc/assets/styles-<hash>.css">`
   * instead of the unprefixed `/assets/styles-<hash>.css`.
   *
   * Accepted shapes (all normalised to a single canonical form
   * internally):
   *
   * - omitted / `undefined` / `""` / `"/"` — no prefix; behaviour is
   *   byte-identical to the pre-`base` build (root-mounted site).
   * - leading-and-trailing-slash path like `"/pj/zudo-doc/"` — prefix
   *   that path onto every asset URL.
   * - absolute URL like `"https://cdn.example.com/"` — emit absolute
   *   URLs (CDN-hosted assets).
   *
   * Inputs missing a leading or trailing `/` are normalised at config-
   * load time (paths) or asset-emit time (URL prefixes); callers do
   * not have to pre-trim.
   *
   * Mirrors `Config::base` in crates/zfb/src/config.rs.
   */
  base?: string;

  /**
   * Canonical origin URL for the site (e.g. `"https://example.com"`).
   *
   * When set, the bundler emits `globalThis.__zfb.site = <value>` in
   * `entry.mjs` so layouts can build canonical `<link>` tags,
   * OpenGraph `og:url` meta, sitemap absolute hrefs, and hreflang
   * `<link rel="alternate">` from a single config-level source of truth.
   *
   * **Distinct from `base`**: `base` is a sub-path mount prefix used
   * for asset URLs (e.g. `"/pj/my-site/"`). `site` is the full
   * canonical origin (scheme + host, no path) used to construct
   * absolute page URLs for SEO/social metadata. Both may be set
   * simultaneously.
   *
   * Accepted shape: an absolute HTTP or HTTPS URL. Relative URLs,
   * non-HTTP(S) schemes, and empty strings are rejected at config-load
   * time. Trailing slash normalisation is the consumer's responsibility.
   *
   * When absent, `globalThis.__zfb.site` is not emitted — the build
   * output is byte-for-byte identical to builds without this field.
   *
   * Mirrors `Config::site` in crates/zfb/src/config.rs.
   */
  site?: string;

  /**
   * Markdown link resolver (port of `remarkResolveMarkdownLinks`).
   *
   * When `enabled: true`, the build appends `ResolveLinksPlugin` to the
   * mdast pipeline so author-written `[label](./other.mdx)` links are
   * rewritten to the corresponding rendered route URL — bypassing the
   * file→directory transformation that breaks relative paths in dist
   * HTML when `foo.mdx` becomes `foo/index.html`. Extensionless
   * (`./other`) and directory-style (`other/`) targets resolve too,
   * probing `{name}.mdx`, `{name}.md`, `{name}/index.mdx`,
   * `{name}/index.md` in that order. Relative targets resolve from the
   * source file's directory; for a directory-style link written from a
   * non-index page against its rendered URL — which sits one directory
   * deeper, e.g. `../sibling/` from `section/article.mdx` — a URL-space
   * fallback retries the probe from the page's route directory when
   * every file-space candidate misses.
   *
   * Two ways to specify the source dirs:
   *
   * - **Single dir (legacy):** set `docsDir` and the build assumes the
   *   `/docs/` route prefix. Convenient for single-locale projects.
   * - **Multi dir (`dirs` non-empty):** explicit `{ dir, routePrefix }`
   *   entries — required for any project with locale mirrors (e.g.
   *   `docs/` AND `docs-ja/`) so each dir maps to its own route prefix
   *   (`/docs/` vs `/ja/docs/`). When `dirs` is non-empty, `docsDir`
   *   is ignored.
   *
   * Mirrors `Config::resolve_markdown_links` in crates/zfb/src/config.rs.
   */
  resolveMarkdownLinks?: ResolveMarkdownLinksConfig;

  /**
   * Whether the basePath rewriter should append a trailing `/` to
   * extensionless absolute hrefs (`<a href="/docs/foo">` becomes
   * `<a href="/pj/zudo-doc/docs/foo/">` when `base = "/pj/zudo-doc/"`
   * and this is `true`).
   *
   * Off by default — preserves byte-for-byte parity with the
   * pre-`trailingSlash` build for projects that haven't opted in.
   * Enable when the deploy target serves canonical URLs with trailing
   * slashes (Cloudflare Pages with `trailingSlash: always`, Netlify
   * pretty URLs, etc.) so the dist HTML doesn't ship non-canonical
   * hrefs that 301-redirect on every click.
   *
   * Only the trailing slash for extensionless hrefs is affected.
   * Hrefs that already end in `/`, that have a file extension
   * (`.png`, `.pdf`, …), or that opt out via `data-no-base` pass
   * through unchanged.
   *
   * Mirrors `Config::trailing_slash` in crates/zfb/src/config.rs.
   */
  trailingSlash?: boolean;

  /**
   * Markdown / MDX parsing options. Currently the only knob exposed is
   * [`gfm`](MarkdownConfig.gfm), which toggles GFM constructs
   * (strikethrough, table, autolink-literal, task-list-item,
   * footnote-definition) on or off.
   *
   * Mirrors `Config::markdown` in crates/zfb/src/config.rs.
   */
  markdown?: MarkdownConfig;

  /**
   * Extra absolute filesystem paths watched by the dev server in
   * addition to the project-root tree.
   *
   * Use this when project content reads from outside the project root
   * (a sibling knowledge-base repo, a shared filesystem directory, a
   * `file:` dep that ships content alongside code, etc.) and you want
   * `zfb dev` to live-reload when those external files change.
   *
   * Semantics:
   *
   * - Each entry MUST be an absolute path. Relative paths are
   *   rejected at config-load time with a clear error message.
   * - Paths are canonicalised when the watcher boots; events match
   *   the canonical form.
   * - A path that does NOT exist at boot is skipped with a warning;
   *   the watcher does NOT re-watch the path if it appears later.
   *   Restart `zfb dev` after creating the path.
   * - Each entry is watched recursively.
   * - Events from outside the project root bypass fine-grained graph
   *   classification and may trigger a broader rebuild than equivalent
   *   in-tree edits.
   *
   * **Security note:** opt-in only — do NOT point this at unbounded
   * directories like `$HOME` or `/`. On Linux the recursive watcher
   * registers every subdirectory and can hit the inotify
   * `max_user_watches` ceiling on large trees.
   *
   * Mirrors `Config::extra_watch_paths` in crates/zfb/src/config.rs.
   */
  extraWatchPaths?: string[];

  /**
   * Whether `zfb build` writes the post-build route manifest to disk
   * at `<outDir>/__zfb/routes.json` (#347).
   *
   * The on-disk file mirrors the in-memory `ctx.routes` shape that the
   * plugin API hands to `postBuild` hooks — same fields, same
   * url-sorted order — so any consumer script wired into `pnpm build`
   * can read the manifest without writing a zfb plugin. The plugin
   * `ctx.routes` and the on-disk `routes.json` are two access shapes
   * over the same data, not two contracts.
   *
   * Default: emit (`undefined` is treated as `true`). Set `false` to
   * skip the write — useful for projects that strip everything but
   * shipped assets out of `dist/` before deploy.
   *
   * Mirrors `Config::emit_routes_manifest` in crates/zfb/src/config.rs.
   */
  emitRoutesManifest?: boolean;

  /**
   * Syntect code-highlight options; absent = default theme
   * (`base16-ocean.dark`) and inline color mode. See
   * {@link CodeHighlightConfig} for accepted theme names, custom-theme
   * loading, and the class-emission mode (Highlight Tokens epic).
   *
   * Mirrors `Config::code_highlight` in crates/zfb/src/config.rs.
   */
  codeHighlight?: CodeHighlightConfig;

  /**
   * Maximum seconds a single plugin lifecycle hook (preBuild, postBuild,
   * setup, etc.) may run before the build fails with a diagnostic error
   * and the plugin host is force-killed.
   *
   * Absent falls through to the `ZFB_PLUGIN_HOOK_TIMEOUT` env var, then
   * the 120s built-in default. Set this when your plugins do long but
   * bounded work (e.g. large sitemap generation) and you want a tighter
   * or more explicit budget.
   *
   * Mirrors `Config::plugin_hook_timeout_secs` in crates/zfb/src/config.rs.
   */
  pluginHookTimeoutSecs?: number;

  /**
   * Whether `copy_public_dir` copies `public/` under the `base`
   * sub-path segment (`true`, default) or flat to the `dist/` root
   * (`false`).
   *
   * - **`true` (default):** files land at
   *   `<outDir>/<base-segment>/<rel>`, matching the base-prefixed URLs
   *   that `withBase()` emits in the rendered HTML. Use this for
   *   projects served directly at their configured sub-path.
   * - **`false`:** files land flat at `<outDir>/<rel>` regardless of
   *   `base`. Use this when the deploy pipeline relocates the entire
   *   `dist/` tree into the base segment itself (e.g.
   *   `cp -a dist/. deploy-root/pj/site/`), so putting the files under
   *   `<outDir>/<base>/...` would result in a double-nested path.
   *
   * **Note on `zfb preview`:** with `false`, base-prefixed public-asset
   * URLs 404 under `zfb preview` because the flat copy lives at the
   * dist root and `zfb preview` does not simulate deploy-side
   * relocation. This is a known trade-off of the flat-copy deploy
   * scheme.
   *
   * Mirrors `Config::copy_public_with_base` in crates/zfb/src/config.rs.
   */
  copyPublicWithBase?: boolean;

  /**
   * Opt into `notify`'s poll-based watch backend for the dev server's
   * watchers instead of the OS-native backend (FSEvents on macOS,
   * inotify on Linux, ...).
   *
   * Use this as a fallback when the native backend is unavailable or
   * unreliable on the host (network-mounted project directories, some
   * CI/sandboxed containers) — the poll backend re-scans the watched
   * roots on an interval instead of relying on OS filesystem-change
   * notifications.
   *
   * Default: `false` (native backend). See
   * {@link watchPollIntervalMs} for the re-scan cadence.
   *
   * Mirrors `Config::watch_poll_fallback` in crates/zfb/src/config.rs.
   */
  watchPollFallback?: boolean;

  /**
   * Re-scan interval, in milliseconds, for the poll watch backend. Only
   * takes effect when {@link watchPollFallback} is `true`.
   *
   * Validated at config-load time: must be between `50` and `10000`
   * (inclusive) — values outside that range are rejected (too low
   * busy-loops the poll thread; too high makes hot-reload feel broken).
   * A value below `100` is accepted but logs a warning (elevated
   * re-scan CPU cost on large trees). Setting this WITHOUT
   * `watchPollFallback: true` is accepted and dormant, with a logged
   * warning rather than an error — a preset may pre-stage the interval
   * ahead of a project enabling the fallback itself.
   *
   * Absent falls through to the built-in 500ms default, applied by the
   * consuming command.
   *
   * Mirrors `Config::watch_poll_interval_ms` in crates/zfb/src/config.rs.
   */
  watchPollIntervalMs?: number;

  /**
   * Project output mode. Drives the V8-mode decision the build engine
   * makes right after the no-SSR-without-adapter precondition check
   * (sub-task 4.1b / issue #373):
   *
   * - `"static"` — declare a pure-static (SSG-only) project. Errors at
   *   build start if any route exports `prerender = false`, pointing
   *   at the offending route. Use this on projects that must never
   *   accidentally pick up an SSR route as a result of a copy-paste.
   * - `"hybrid"` — declare a project that may host SSR routes. V8-on
   *   regardless of detection, even when no `prerender = false` route
   *   currently exists. Useful for projects that will add SSR routes
   *   later and want a stable build topology in the meantime.
   * - `"auto"` (default) — detection-driven. Non-empty `prerender =
   *   false` route set => V8-on; empty => V8-off.
   *
   * Today's load-bearing role is the `"static"` precondition check.
   * The V8-off branch does NOT skip V8 host startup on the shipping
   * `zfb` binary — SSG still needs V8 to render pages. The flag exists
   * as infrastructure for the future shipping path (Tauri sidecar /
   * standalone SSR server). See the
   * [Build engine docs](https://github.com/Takazudo/zudo-front-builder/blob/main/docs/src/content/docs/architecture/build-engine.mdx)
   * for the gate decision table.
   *
   * Mirrors `Config::output` in crates/zfb/src/config.rs.
   */
  output?: OutputMode;

  /**
   * Config presets to merge before validation (#1196).
   *
   * Each preset is a partial `ZfbConfig`-shaped object. The merge pass runs
   * BEFORE field validation and folds preset contributions using additive
   * semantics:
   *
   * - **Array fields** (`plugins`, `collections`, `extraWatchPaths`,
   *   `allowedHosts`): preset values are prepended so the main config's
   *   entries retain their relative position after the preset's.
   * - **Scalar / optional fields**: a preset value fills in only when the
   *   main config leaves the field at its default — the main config is
   *   authoritative; presets act as defaults.
   *
   * Nested `presets` inside a preset are NOT recursively expanded.
   *
   * Mirrors `Config::presets` in crates/zfb/src/config.rs.
   */
  presets?: Partial<ZfbConfig>[];
};

/**
 * Project output mode.
 *
 * - `"static"` — pure-static (SSG-only); errors on detected SSR routes.
 * - `"hybrid"` — may host SSR routes; V8-on regardless of detection.
 * - `"auto"` — detection-driven; the default.
 *
 * Mirrors `OutputMode` in crates/zfb/src/config.rs.
 */
export type OutputMode = "static" | "hybrid" | "auto";

/**
 * Syntect code-highlight options.
 *
 * Unknown theme names are rejected at build start with a clear error
 * rather than silently falling back.
 *
 * **Single-theme mode** (the default): set `theme` to a syntect theme name,
 * or omit it to use the default (`"base16-ocean.dark"`). Tokens are colored
 * with inline `color:`.
 *
 * **Dual-theme mode**: set both `themeLight` and `themeDark`. Tokens are
 * colored with CSS custom properties (`--shiki-light` / `--shiki-dark`),
 * and the consumer applies a `light-dark()` rule to pick the active color.
 * The `<pre>` element carries `class="syntect-dual"` and
 * `--shiki-light-bg` / `--shiki-dark-bg` in its `style` attribute.
 *
 * `theme` and the dual pair are mutually exclusive. Setting only one of
 * `themeLight` / `themeDark` is an error.
 *
 * All theme names are **SYNTECT** built-in or user-loaded names (e.g.
 * `"base16-ocean.light"`, `"base16-ocean.dark"`, `"InspiredGitHub"`,
 * `"Solarized (dark)"`), NOT Shiki names like `"dracula"`.
 *
 * **Class mode** (Highlight Tokens epic, zfb#1528): set `mode: "class"`.
 * Each token gets a semantic role class instead of an inline color, so
 * highlight colors become re-themeable CSS design tokens. Mutually
 * exclusive with `theme` / `themeLight` / `themeDark` / `themesDir` —
 * themes don't affect class emission, so setting both is a build error.
 *
 * Mirrors `CodeHighlightConfig` in crates/zfb/src/config.rs.
 */
export type CodeHighlightConfig = {
  /**
   * Syntect built-in or user-loaded theme name. When absent the
   * pipeline defaults to `"base16-ocean.dark"`.
   *
   * Mutually exclusive with {@link themeLight} / {@link themeDark}.
   * Must be a SYNTECT theme name (e.g. `"InspiredGitHub"`), NOT a Shiki name.
   */
  theme?: string;
  /**
   * Path to a directory of `.tmTheme` files, relative to the project
   * root. Every `.tmTheme` file in the directory is loaded and becomes
   * available by its declared `name` via {@link theme}, {@link themeLight},
   * or {@link themeDark}. When absent only syntect's bundled themes are
   * available.
   *
   * The path must be relative and must not escape the project root via
   * `..`. A missing directory is reported as an error at build start.
   *
   * Applies to both single-theme and dual-theme mode.
   */
  themesDir?: string;
  /**
   * Light-mode syntect theme name for dual-theme highlighting.
   *
   * Must be set together with {@link themeDark} — setting only one of
   * the two is a build error. When both are set, tokens are colored with
   * CSS custom properties (`--shiki-light` / `--shiki-dark`) instead of
   * inline `color:`. Mutually exclusive with {@link theme}.
   *
   * Must be a SYNTECT theme name (e.g. `"base16-ocean.light"`),
   * NOT a Shiki name like `"dracula"`.
   */
  themeLight?: string;
  /**
   * Dark-mode syntect theme name for dual-theme highlighting.
   *
   * Must be set together with {@link themeLight} — setting only one of
   * the two is a build error. Mutually exclusive with {@link theme}.
   *
   * Must be a SYNTECT theme name (e.g. `"base16-ocean.dark"`),
   * NOT a Shiki name like `"dracula"`.
   */
  themeDark?: string;
  /**
   * Output mode for fenced-code highlighting (Highlight Tokens epic,
   * zfb#1528). `"inline"` (default) bakes per-token colors into
   * `style="color:#rrggbb"` (or the dual `--shiki-*` custom properties).
   * `"class"` emits a semantic role class per token instead, so colors
   * become re-themeable CSS design tokens rather than baked-in HTML.
   *
   * Mutually exclusive with {@link theme} / {@link themeLight} /
   * {@link themeDark} / {@link themesDir} — themes don't affect class
   * emission, so setting both is rejected rather than silently ignoring
   * the theme.
   */
  mode?: CodeHighlightMode;
  /**
   * Class-name prefix for class-mode role classes (e.g. the default
   * `"hi-"` yields `hi-kw`, `hi-str`, ...). Must match
   * `/^[A-Za-z][A-Za-z0-9_-]*$/`. Only meaningful when {@link mode} is
   * `"class"`. Default: `"hi-"`.
   */
  classPrefix?: string;
  /**
   * Per-role class overrides for class mode, e.g.
   * `{ keyword: "text-violet-600 dark:text-violet-400" }` to map a role
   * onto Tailwind utilities instead of the default `{classPrefix}{role}`
   * class. Keys must be one of the 18 fixed role names (see
   * {@link CodeHighlightRole}); a value may hold multiple
   * space-separated classes and must not contain the bare token `"line"`
   * (collides with the code-enrichment line wrapper class). Absent uses
   * `{classPrefix}{role}` for every role.
   *
   * Setting this while `tailwind.enabled` is `false` (the authored-CSS
   * path) is allowed but emits a build warning — no Tailwind safelist can
   * be generated on that path, so the mapped utilities must already exist
   * in your own CSS.
   */
  roleClasses?: Partial<Record<CodeHighlightRole, string>>;
  /**
   * Whether to inject the built-in `--zfb-hi-*` token stylesheet
   * (`zfb-hi.css`) into the combined `styles.css` output. Only meaningful
   * in class mode. Default: `true`.
   */
  defaultStylesheet?: boolean;
};

/**
 * `codeHighlight.mode` — see {@link CodeHighlightConfig.mode}.
 *
 * Mirrors `CodeHighlightMode` in crates/zfb/src/config.rs.
 */
export type CodeHighlightMode = "inline" | "class";

/**
 * The fixed 18-role semantic taxonomy for class-mode syntax highlighting
 * (Highlight Tokens epic, zfb#1528) — valid {@link CodeHighlightConfig.roleClasses}
 * keys.
 *
 * Mirrors `CODE_HIGHLIGHT_ROLES` in crates/zfb/src/config.rs.
 */
export type CodeHighlightRole =
  | "escape"
  | "operator"
  | "comment"
  | "string"
  | "number"
  | "constant"
  | "keyword"
  | "function"
  | "type"
  | "namespace"
  | "property"
  | "variable"
  | "tag"
  | "attribute"
  | "punctuation"
  | "inserted"
  | "deleted"
  | "heading";

/**
 * Table-of-contents options. Wire via `markdown.toc` in `zfb.config.ts`.
 *
 * When present, a TOC `<ul>/<li>` list is inserted as the next sibling
 * of the first heading whose text matches `heading` (case-insensitive).
 * Each `<a href="#id">` links to the deduplicated `id` that
 * `HeadingLinksPlugin` placed on the corresponding heading.
 *
 * Mirrors `TocConfig` in `crates/zfb-content/src/plugins/toc.rs`.
 */
export type TocConfig = {
  /**
   * Heading text that triggers TOC insertion. Matched
   * case-insensitively after whitespace trimming. Default: `"TOC"`.
   */
  heading?: string;

  /**
   * Number of heading levels to include starting from `<h2>`.
   *
   * - `1` — h2 only
   * - `2` (default) — h2 + h3
   * - `3` — h2, h3, h4
   * - …up to `5` (h2 through h6)
   */
  maxDepth?: number;
};

/**
 * Markdown / MDX parsing options.
 *
 * See [`ZfbConfig.markdown`] for the embed point. Fields: [`gfm`],
 * [`toc`], [`externalLinks`], [`cjkFriendly`], and [`features`].
 * Future markdown knobs would also live here.
 *
 * See the "Markdown Features" docs category for the per-feature option
 * reference once individual features are ported.
 *
 * Mirrors `MarkdownConfig` in crates/zfb/src/config.rs.
 */
export type MarkdownConfig = {
  /**
   * Enable GFM constructs.
   *
   * Accepts three shapes:
   *
   * - `true` — turn every GFM construct ON (strikethrough, table,
   *   autolink-literal, task-list-item, footnote-definition).
   * - `false` — turn every GFM construct OFF.
   * - partial object — set individual fields explicitly; fields you
   *   omit fall back to the conservative-default values described
   *   below.
   *
   * When `markdown` itself is omitted entirely, the conservative
   * default applies: `strikethrough: true`, `table: true`, every other
   * GFM construct off. This is the smallest behavioural delta from
   * zfb's historical effective state (table-only). Projects that want
   * the full GFM surface should opt in with `gfm: true`.
   */
  gfm?: GfmFlag;

  /**
   * Table-of-contents options. When present, a `<ul>/<li>` list is
   * inserted after the first heading whose text matches `heading`
   * (default `"TOC"`, case-insensitive). Each link points to the
   * deduplicated `id` that `HeadingLinksPlugin` placed on the heading.
   *
   * Omitting this field entirely leaves the build byte-for-byte identical
   * to the pre-TOC build. See [`TocConfig`] for the available options.
   *
   * Mirrors `MarkdownConfig::toc` in crates/zfb/src/config.rs.
   */
  toc?: TocConfig;
  /**
   * External-link rewriter. When set, every `<a>` whose href is
   * classified as external receives the configured `target` and `rel`
   * attributes.
   *
   * An href is external when it is an absolute HTTP/HTTPS URL AND its
   * origin differs from the top-level `site` URL (if `site` is
   * configured). When `site` is absent, any absolute HTTP/HTTPS URL is
   * treated as external.
   *
   * `mailto:`, `tel:`, and other non-HTTP(S) schemes are always left
   * unchanged. Relative URLs (`/internal/`, `./file.mdx`, `#anchor`) are
   * always internal.
   *
   * Omitting this field keeps the output byte-for-byte identical to the
   * pre-feature behaviour.
   *
   * Mirrors `ExternalLinksConfig` in crates/zfb/src/config.rs.
   */
  externalLinks?: ExternalLinksConfig;

  /**
   * Enable CJK-friendly markdown handling.
   *
   * Governs two post-parse fixups that adapt CommonMark/GFM rules to CJK
   * text:
   *
   * 1. **Emphasis/strong flanking** (`CjkFriendlyPlugin`). CommonMark's
   *    left-/right-flanking delimiter-run rules treat CJK characters as
   *    non-whitespace non-punctuation, which causes `**foo**` adjacent to
   *    CJK text (e.g. `**テスト。**テスト`) to render as literal stars
   *    instead of `<strong>`.
   * 2. **Bare-URL autolink boundary** (`CjkAutolinkBoundaryPlugin`,
   *    zfb#1105). The GFM autolink-literal path grammar terminates only on
   *    ASCII whitespace, so a bare URL flush against CJK text
   *    (`詳細はhttps://example.com参照`) swallows the trailing CJK run into
   *    the `href`. This fixup terminates the link at the first CJK
   *    character. Only active when `gfm.autolinkLiteral` is also on.
   *
   * - **absent / `true` (default):** CJK-friendly handling is on.
   *   Preserves today's behaviour — existing CJK-content sites are
   *   unaffected.
   * - **`false`:** opt-out. Neither plugin is added to the pipeline;
   *   emphasis markers and bare-URL autolinks adjacent to CJK characters
   *   follow base CommonMark/GFM rules. Rarely the right choice; provided
   *   as an escape hatch for projects that need strict CommonMark/GFM
   *   output.
   *
   * **GFM strikethrough** (`~~foo~~`) at CJK boundaries is unaffected
   * by this toggle — it is handled by markdown-rs's GFM tokeniser, not
   * by these plugins, and works correctly in both modes.
   *
   * Mirrors `MarkdownConfig::cjk_friendly` in crates/zfb/src/config.rs.
   */
  cjkFriendly?: boolean;

  /**
   * Convert every soft line break (a single `\n` inside a paragraph) into
   * `<br>` (remark-breaks parity).
   *
   * - **absent / `false` (default):** soft line breaks follow standard
   *   CommonMark behaviour — collapsed into a single space.
   * - **`true`:** every `\n` inside a paragraph becomes `<br>`. Use this
   *   when your content relies on newline→`<br>` fidelity (e.g. product
   *   descriptions, lyrics, or other newline-sensitive prose).
   *
   * Mirrors `MarkdownConfig::hard_breaks` in crates/zfb/src/config.rs.
   */
  hardBreaks?: boolean;

  /**
   * Per-feature markdown pipeline toggles.
   *
   * Each field is a [`FeatureToggle`] (`true` / `false` / options object)
   * or a feature-specific config type (for features that require extra
   * parameters). Absent / `undefined` means all features are disabled,
   * preserving the behaviour of the pre-features build byte-for-byte.
   *
   * Unknown keys are rejected at deserialization time by the Rust loader
   * so a typo in `zfb.config.ts` surfaces as a clear error.
   *
   * Mirrors `MarkdownFeaturesConfig` in crates/zfb/src/config.rs.
   */
  features?: MarkdownFeaturesConfig;
};

/**
 * Per-feature toggle: `boolean` shorthand or an options object.
 *
 * `true` enables the feature with defaults; `false` (or absent) disables it.
 * The object form carries per-feature options (fields vary by feature and
 * are filled in by each feature's port sub-issue — stubs today).
 *
 * Mirrors `FeatureToggle` in crates/zfb/src/config.rs.
 */
export type FeatureToggle = boolean | FeatureOptions;

/**
 * Empty options object for features that accept `{ ... }` but have no
 * user-facing knobs yet. Fields are filled in by each feature's port
 * sub-issue; this stub satisfies the schema shape requirement.
 *
 * Mirrors `FeatureOptions` in crates/zfb/src/config.rs.
 */
export type FeatureOptions = Record<string, never>;

/**
 * Options for the `codeEnrichment` feature.
 *
 * All flags default to `true` when the feature is enabled with
 * `codeEnrichment: {}` or when a field is absent.
 *
 * Mirrors `CodeEnrichmentConfig` in `crates/zfb-md-ast/src/features_config.rs`.
 */
export type CodeEnrichmentConfig = {
  /**
   * Enable diff-marker processing for markers such as `// [!code ++]`
   * and `// [!code --]`. Default: `true`.
   */
  diffMarkers?: boolean;
  /**
   * Enable line-highlight processing for fence ranges such as `{1,3-5}`.
   * Default: `true`.
   */
  lineHighlight?: boolean;
  /**
   * Enable visible-text word emphasis for slash-delimited fence metadata
   * such as `/answer/`. Default: `true`.
   */
  wordHighlight?: boolean;
};

/**
 * Options for the `tocExport` feature.
 *
 * Controls which headings are included in the exported `toc` JSON.
 * `maxDepth` is the **absolute** heading depth (2–6):
 *   - `2` → h2 only
 *   - `3` (default) → h2 + h3
 *
 * This differs from `headingMarkerToc.maxDepth`, which counts levels
 * starting from h2. The two features are independent.
 *
 * Mirrors `TocExportConfig` in crates/zfb-md-ast/src/features_config.rs.
 */
export type TocExportConfig = {
  /** Maximum heading depth to include (absolute, 2–6). Default: 3. */
  maxDepth?: number;
};

/**
 * Options for the `imageDimensions` feature.
 *
 * Auto-detects and injects `width`/`height` on local `<img>` elements. Raster
 * formats are probed header-only; SVGs are read from their markup
 * (`width`/`height`/`viewBox`).
 *
 * Mirrors `ImageDimensionsConfig` in crates/zfb-md-ast/src/features_config.rs.
 */
export type ImageDimensionsConfig = {
  /**
   * When `true` (the default), `http://` and `https://` image sources are
   * silently skipped and not probed for dimensions. Set to `false` only for
   * testing or unusual setups — remote images require network access at build
   * time and slow the pipeline.
   */
  skipRemote?: boolean;
};

/**
 * Options for the `linkValidation` feature.
 *
 * Validates internal `[text](file.md#anchor)` and `[text](#anchor)` links at
 * build time. External URLs (`http://`, `https://`, `mailto:`) are always
 * skipped — network validation is out of scope.
 *
 * Mirrors `LinkValidationConfig` in `crates/zfb-md-ast/src/features_config.rs`.
 */
export type LinkValidationConfig = {
  /**
   * When `true`, broken links are reported as errors (build can fail).
   * Default: `false` (warn-only).
   */
  failOnBroken?: boolean;
};

/**
 * Options for the `transclude` feature.
 *
 * Enables `:::include{file="./path.md"}` directives that inline another
 * file's parsed mdast at the include site.
 *
 * Mirrors `TranscludeConfig` in crates/zfb-md-ast/src/features_config.rs.
 */
export type TranscludeConfig = {
  /**
   * Maximum transclusion depth (chain length A→B→C→…).
   *
   * A depth of `1` allows only direct includes (the included file itself
   * cannot include further files). Default: `5`. A cycle (A→B→A) is
   * always detected regardless of `maxDepth` and treated as an error.
   */
  maxDepth?: number;
};

/**
 * Options for the `readingTime` feature.
 *
 * Mirrors `ReadingTimeOptions` in crates/zfb-md-ast/src/features_config.rs.
 */
export type ReadingTimeConfig = {
  /** Words-per-minute rate for the reading-time estimate. Default: 200. */
  wpm?: number;
};

/**
 * `readingTime` feature value: either a `boolean` shorthand or a
 * {@link ReadingTimeConfig} options object.
 *
 * Mirrors `ReadingTimeFeature` in crates/zfb-md-ast/src/features_config.rs.
 */
export type ReadingTimeFeature = boolean | ReadingTimeConfig;

/**
 * Per-feature markdown pipeline configuration.
 *
 * All fields are optional; absent = feature disabled, behaviour unchanged
 * from the pre-features build. Unknown keys are rejected at deserialization
 * time by the Rust loader so a typo surfaces as a clear error.
 *
 * Mirrors `MarkdownFeaturesConfig` in crates/zfb/src/config.rs.
 */
export type MarkdownFeaturesConfig = {
  /** GitHub-style alert blocks (`> [!NOTE]`, `> [!WARNING]`, etc.). */
  githubAlerts?: FeatureToggle;

  /**
   * Reading-time estimate injected into the document frontmatter.
   * Accepts `true` / `false` shorthand or `{ wpm: N }` for a custom rate.
   */
  readingTime?: ReadingTimeFeature;

  /** Code-block enrichment (copy button, language label, etc.). */
  codeEnrichment?: CodeEnrichmentConfig;

  /** Grouped code blocks rendered as tabs. */
  codeTabs?: FeatureToggle;

  /** Ruby annotation support (`{base}^{ruby}` syntax). */
  ruby?: FeatureToggle;

  /** Export the page TOC as structured data (e.g. for sidebar rendering). */
  tocExport?: TocExportConfig;

  /** Auto-detect and inject `width`/`height` on `<img>` elements. */
  imageDimensions?: ImageDimensionsConfig;

  /**
   * Validate internal links (file-relative paths and anchor fragments) at
   * build time. External URLs are always skipped — network validation is
   * out of scope.
   */
  linkValidation?: LinkValidationConfig;

  /**
   * Transclusion of other markdown/MDX files via
   * `:::include{file="./path.md"}` — NOT the Obsidian `[[path]]` wikilink
   * syntax.
   */
  transclude?: TranscludeConfig;

  /**
   * Generic `:::name` → component map. You supply the components; no defaults
   * are registered. Keys are directive names (e.g. `"foo"`), values are
   * {@link DirectiveSpec} (bare component name string or options object).
   *
   * Mirrors `directives` in `MarkdownFeaturesConfig` in crates/zfb/src/config.rs.
   */
  directives?: Record<string, DirectiveSpec>;

  /** Mermaid diagram rendering. */
  mermaid?: FeatureToggle;

  /**
   * Inline heading-marker TOC. Accepts either a `boolean` shorthand
   * (`true` = enable with defaults, `false` = disable) or a full
   * {@link TocConfig} options object — same union shape as the Rust
   * `HeadingMarkerTocFeature` enum.
   */
  headingMarkerToc?: HeadingMarkerTocFeature;

  /**
   * Heading-ID strategy for the always-on `HeadingLinks` plugin.
   * Absent → `"flat"` (the long-standing github-slugger scheme).
   * `{ strategy: "hierarchical" }` opts into ancestor-prefixed anchor
   * IDs (`## Foo` / `### Moo` / `#### Mew` → `foo`, `foo-moo`,
   * `foo-moo-mew`) — see {@link HeadingIdsConfig}.
   */
  headingIds?: HeadingIdsConfig;
};

/**
 * Options for the `headingIds` entry in `markdown.features`.
 *
 * Configures the always-on `HeadingLinks` plugin rather than toggling an
 * opt-in feature. Note: switching to `"hierarchical"` is anchor-breaking
 * for existing deep links to nested headings.
 *
 * Mirrors `HeadingIdsConfig` in crates/zfb-md-ast/src/features_config.rs.
 */
export type HeadingIdsConfig = {
  /**
   * `"flat"` (default): github-slugger slugs with a per-document dedup
   * counter shared across h2–h6 (`overview`, `overview-1`, …).
   * `"hierarchical"`: each heading's slug is prefixed with its ancestor
   * chain and deduped on the full path — anchors become reconstructible
   * from the heading outline.
   */
  strategy?: "flat" | "hierarchical";
};

/**
 * `headingMarkerToc` feature value: either a `boolean` shorthand or a
 * full {@link TocConfig} options object.
 *
 * Mirrors `HeadingMarkerTocFeature` in crates/zfb-md-ast/src/features_config.rs.
 */
export type HeadingMarkerTocFeature = boolean | TocConfig;

/**
 * Spec for one user-defined directive: either a bare component name string
 * or a full {@link DirectiveFullSpec} options object.
 *
 * Mirrors `DirectiveSpec` in crates/zfb-md-ast/src/features_config.rs.
 */
export type DirectiveSpec = string | DirectiveFullSpec;

/**
 * Full options object for one user-defined directive.
 *
 * Mirrors `DirectiveFullSpec` in crates/zfb-md-ast/src/features_config.rs.
 */
export type DirectiveFullSpec = {
  /** JSX component identifier (e.g. `"Spoiler"`, `"Kbd"`). */
  component: string;
  /** Container/leaf/text shape. Defaults to `"container"` when absent. */
  kind?: "container" | "leaf" | "text";
  /** Whether the bracketed `[label]` becomes a `title` attribute. Defaults to `true`. */
  titleFromLabel?: boolean;
};

/**
 * Options for the external-link rewriter (port of `rehype-external-links`).
 *
 * All fields are optional; omitting a field applies the documented default.
 *
 * Mirrors `ExternalLinksConfig` in crates/zfb/src/config.rs.
 */
export type ExternalLinksConfig = {
  /**
   * `rel` tokens applied to external links.
   *
   * Default: `["noopener", "noreferrer"]`.
   *
   * Tokens are deduplicated (case-insensitive) and merged with any
   * existing `rel` attribute on the `<a>` element — existing tokens
   * appear first.
   */
  rel?: string[];
  /**
   * `target` value for external links.
   *
   * Default: `"_blank"`.
   */
  target?: string;
};

/**
 * Either the shorthand boolean form (`true` = all GFM constructs on,
 * `false` = all off) or a partial object that toggles individual
 * constructs.
 *
 * Mirrors `GfmFlag` in crates/zfb/src/config.rs.
 */
export type GfmFlag = boolean | GfmConstructs;

/**
 * Per-construct opt-in / opt-out for GFM. Every field is optional;
 * omitted fields fall back to the conservative default
 * (`strikethrough: true`, `table: true`, others `false`).
 *
 * Mirrors `GfmConstructs` in crates/zfb/src/config.rs.
 */
export type GfmConstructs = {
  /** GFM strikethrough (`~~text~~` → `<del>text</del>`). */
  strikethrough?: boolean;
  /** GFM pipe-style tables. */
  table?: boolean;
  /**
   * GFM autolink literal — bare URLs like `https://example.com` become
   * clickable links without `<…>` brackets.
   */
  autolinkLiteral?: boolean;
  /** GFM task list items (`- [x]` / `- [ ]`). */
  taskListItem?: boolean;
  /** GFM footnote definitions (`[^ref]: …`). */
  footnoteDefinition?: boolean;
};

/**
 * What to do when a `.md`/`.mdx` link cannot be resolved.
 *
 * Mirrors `OnBrokenLinks` in crates/zfb/src/config.rs.
 */
export type OnBrokenLinks = "warn" | "error" | "ignore";

/**
 * Config for the markdown link resolver. See
 * [`ZfbConfig.resolveMarkdownLinks`] for the design rationale.
 */
export type ResolveMarkdownLinksConfig = {
  /** Whether to enable link resolution. Default: `false`. */
  enabled?: boolean;

  /**
   * Legacy single-dir field. Used only when [`dirs`] is empty. When
   * non-empty, scanned against the hard-coded `/docs/` route prefix.
   */
  docsDir?: string;

  /**
   * Explicit per-dir source map. Each entry is one collection (e.g.
   * EN docs at `src/content/docs/` → `/docs/`, JA docs at
   * `src/content/docs-ja/` → `/ja/docs/`). Takes precedence over
   * [`docsDir`] when non-empty.
   */
  dirs?: ResolveMarkdownLinksDir[];

  /** What to do with unresolved `.md`/`.mdx` links. Default: `"warn"`. */
  onBrokenLinks?: OnBrokenLinks;
};

/** One source-dir entry for [`ResolveMarkdownLinksConfig.dirs`]. */
export type ResolveMarkdownLinksDir = {
  /**
   * Directory (relative to project root) whose `.md`/`.mdx` files are
   * scanned. Must be relative and must not escape the root via `..`.
   */
  dir: string;

  /**
   * Route prefix prepended to each file's slug. Include leading and
   * trailing slashes (e.g. `"/docs/"` or `"/ja/docs/"`).
   */
  routePrefix: string;
};

/**
 * Identity helper: returns the supplied config as-is, but typed against
 * [`ZfbConfig`]. Use as the default export of `zfb.config.ts` so editors
 * surface field-level types and typos surface at compile time.
 */
export function defineConfig(config: ZfbConfig): ZfbConfig {
  return config;
}

/**
 * Preset authoring helper: stamps each object entry in `config.plugins`
 * with `source_package: sourcePackage` so the Rust loader can attribute
 * plugin contributions back to the preset package that provided them.
 *
 * - Only plain-object plugin entries are stamped; non-object entries pass
 *   through unchanged (defensive — the current schema requires objects,
 *   but this guard keeps the helper safe if the schema is ever relaxed).
 * - An entry that ALREADY carries a `source_package` is left untouched, so a
 *   preset composing another `definePreset`-returned preset (by spreading its
 *   `plugins`) keeps the inner preset's provenance instead of clobbering it
 *   with the outer package name (the spread below lets the existing marker win).
 * - When `config.plugins` is absent, the config is returned as-is.
 * - All other fields of `config` pass through unchanged.
 *
 * The key `source_package` (snake_case) mirrors the Rust `PluginConfig`
 * serde field added in T4. `PluginConfig` has no `#[serde(rename_all)]`
 * so the serde key is the field name verbatim — do NOT use camelCase.
 *
 * SYNC REQUIREMENT: keep this implementation behaviourally identical to
 * the stub in crates/zfb-config-loader/js/zfb-config-stub.mjs, which is
 * injected at config-eval time when the user's project does not have the
 * zfb npm package installed locally.
 */
export function definePreset(
  sourcePackage: string,
  config: Partial<ZfbConfig>,
): Partial<ZfbConfig> {
  if (!config.plugins) {
    return config;
  }
  return {
    ...config,
    plugins: config.plugins.map((plugin) => {
      if (plugin !== null && typeof plugin === "object" && !Array.isArray(plugin)) {
        // Default first, then spread the plugin so an existing `source_package`
        // (from a composed inner preset) wins over the outer package name.
        return { source_package: sourcePackage, ...plugin };
      }
      return plugin;
    }),
  };
}
