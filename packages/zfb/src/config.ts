// `zfb/config` — TypeScript helper for the `zfb.config.ts` form.
//
// The zfb config loader (`crates/zfb/src/config.rs`) accepts both
// `zfb.config.ts` and `zfb.config.json`; JSON wins when both files are
// present, which is the back-compat path for projects predating the TS
// loader. New projects should prefer the TS form for editor types and
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
  /** Optional schema. Reserved for v1.1 — accepted but not enforced today. */
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
 * One plugin entry in `zfb.config.ts`.
 *
 * `name` MUST be a module reference that Node's resolver can locate from
 * the project root. The zfb config loader (`crates/zfb/js/config-loader.mjs`)
 * resolves it to an absolute module specifier and the build / dev plugin
 * host loads it via dynamic `import()`:
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
  /** User-supplied plugins. */
  plugins?: PluginConfig[];
  /**
   * Deploy-target adapter package name. Omit (or `"none"`) for a pure
   * static build — any route exporting `prerender = false` is then a
   * hard build error. A package name like
   * `"@takazudo/zfb-adapter-cloudflare"` selects the matching adapter,
   * and `zfb build` invokes that package's bin to wrap the SSR bundle
   * into a deploy-ready entry (e.g. `dist/_worker.js` for Cloudflare
   * Pages).
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
   * HTML when `foo.mdx` becomes `foo/index.html`.
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
   * Enable CJK-friendly emphasis/strong re-tokenisation.
   *
   * CommonMark's left-/right-flanking delimiter-run rules treat CJK
   * characters as non-whitespace non-punctuation, which causes `**foo**`
   * adjacent to CJK text (e.g. `**テスト。**テスト`) to render as literal
   * stars instead of `<strong>`. zfb's built-in `CjkFriendlyPlugin`
   * corrects this post-parse.
   *
   * - **absent / `true` (default):** CJK-friendly re-tokenisation is
   *   on. Preserves today's behaviour — existing CJK-content sites are
   *   unaffected.
   * - **`false`:** opt-out. `CjkFriendlyPlugin` is NOT added to the
   *   pipeline; emphasis markers adjacent to CJK characters follow base
   *   CommonMark flanking rules. Rarely the right choice; provided as
   *   an escape hatch for projects that need strict CommonMark output.
   *
   * **GFM strikethrough** (`~~foo~~`) at CJK boundaries is unaffected
   * by this toggle — it is handled by markdown-rs's GFM tokeniser, not
   * by `CjkFriendlyPlugin`, and works correctly in both modes.
   *
   * Mirrors `MarkdownConfig::cjk_friendly` in crates/zfb/src/config.rs.
   */
  cjkFriendly?: boolean;

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
 * Options for the `githubAutolinks` feature. Requires `repo`.
 *
 * TODO: fill in actual fields when the githubAutolinks feature is ported.
 *
 * Mirrors `GithubAutolinksConfig` in crates/zfb/src/config.rs.
 */
export type GithubAutolinksConfig = {
  /** GitHub repository reference (`owner/repo`) used to build autolink URLs. */
  repo?: string;
};

/**
 * Options stub for the `codeEnrichment` feature.
 *
 * TODO: fill in actual fields when the codeEnrichment feature is ported.
 *
 * Mirrors `CodeEnrichmentConfig` in crates/zfb/src/config.rs.
 */
export type CodeEnrichmentConfig = Record<string, never>;

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
 * Options stub for the `imageDimensions` feature.
 *
 * TODO: fill in actual fields when the imageDimensions feature is ported.
 *
 * Mirrors `ImageDimensionsConfig` in crates/zfb/src/config.rs.
 */
export type ImageDimensionsConfig = Record<string, never>;

/**
 * Options stub for the `linkValidation` feature.
 *
 * TODO: fill in actual fields when the linkValidation feature is ported.
 *
 * Mirrors `LinkValidationConfig` in crates/zfb/src/config.rs.
 */
export type LinkValidationConfig = Record<string, never>;

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

  /** Reading-time estimate injected into the document frontmatter. */
  readingTime?: FeatureToggle;

  /** GitHub-style `owner/repo#123` and `SHA` autolinks. Requires `repo`. */
  githubAutolinks?: GithubAutolinksConfig;

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

  /** Validate internal and external links at build time. */
  linkValidation?: LinkValidationConfig;

  /** Transclusion of other MDX files (`![[path]]` syntax). */
  transclude?: TranscludeConfig;

  /** Framework admonitions preset (maps `:::note` etc. to components). */
  admonitionsPreset?: FeatureToggle;

  /** Mermaid diagram rendering. */
  mermaid?: FeatureToggle;

  /** Lightbox / image-enlarge on click. */
  imageEnlarge?: FeatureToggle;

  /**
   * Inline heading-marker TOC. Accepts either a `boolean` shorthand
   * (`true` = enable with defaults, `false` = disable) or a full
   * {@link TocConfig} options object — same union shape as the Rust
   * `HeadingMarkerTocFeature` enum.
   */
  headingMarkerToc?: HeadingMarkerTocFeature;
};

/**
 * `headingMarkerToc` feature value: either a `boolean` shorthand or a
 * full {@link TocConfig} options object.
 *
 * Mirrors `HeadingMarkerTocFeature` in crates/zfb-md-ast/src/features_config.rs.
 */
export type HeadingMarkerTocFeature = boolean | TocConfig;

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
