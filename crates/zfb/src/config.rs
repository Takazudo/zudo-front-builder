//! Project configuration loader.
//!
//! Loads `zfb.config.ts` or `zfb.config.json` from the project root and
//! parses it into a strongly-typed [`Config`] struct.
//!
//! ## Bootstrap rule
//!
//! The JS runtime used to *parse* this config is fixed by the zfb binary
//! itself — the config CANNOT choose its own runtime. The config CAN choose
//! `framework: "preact" | "react"` (applied after the config is loaded by the
//! framework adapter), `outDir`, `publicDir`, content + Tailwind options, and
//! plugins. There is exactly one runtime; it is not user-overridable in v1.
//!
//! See issue #9 (Wave 2 / Sub 3).
//!
//! ## Resolution order
//!
//! [`load_from_dir`] picks exactly one source per call:
//!
//! 1. `dir/zfb.config.ts` — bundled to ESM by the pinned `esbuild` binary
//!    (the same one zfb-islands uses), then evaluated **in-process** by the
//!    embedded V8 isolate (default builds). The default export is returned as
//!    a JSON envelope and fed into `serde_json::from_str`.
//!    **TS wins over JSON when both files are present** — the TS form is the
//!    canonical, recommended way to author a zfb config.
//! 2. `dir/zfb.config.json` — read + parse via `serde_json`. Used only when
//!    no `zfb.config.ts` is found.
//! 3. Neither present — return [`Config::default`].
//!
//! `zfb.config.ts` is evaluated in-process via the embedded V8 isolate that
//! ships with the default `zfb` binary. There is no runtime Node dependency.
//! `zfb.config.ts` is a data config — `node:*` imports, `process.env`, and
//! other Node-only APIs are not available inside the evaluator (esbuild
//! `--platform=neutral` rejects them at bundle time).
//!
//! **Slim-build fallback** (`--no-default-features` / no `embed_v8`): the
//! in-process evaluator is compiled out; TS config evaluation falls back to
//! the `node` subprocess path. This path requires `node` in `PATH` and
//! surfaces a clean error when it is absent.
//!
//! The TS evaluator itself (esbuild bundle + V8 / node) lives in the shared
//! leaf crate [`zfb_config_loader`] (issue #1037); this module wraps it,
//! deserialising the evaluated value into the strongly-typed [`Config`].
//!
//! All produced configs pass [`validate`] before they are returned so
//! callers don't have to think about it.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{de, Deserialize, Deserializer, Serialize};

// OsString is used by LoadOptions::node_binary (always compiled in).
use std::ffi::OsString;

// Canonical default slot path — defined once in zfb-build, imported here so
// the slim-build node-not-found test can reference it without repeating the
// string literal. Only the no-embed_v8 test item uses it, so the import is
// gated the same way.
#[cfg(all(test, not(feature = "embed_v8")))]
use zfb_build::DEFAULT_ESBUILD_SLOT;

// --- JsonSchema newtype --------------------------------------------------------

/// A validated JSON Schema document for a content collection.
///
/// Wraps a [`serde_json::Value`] that has been checked at config-load time
/// to be a well-formed schema object. The check is intentionally minimal —
/// it mirrors the dialect accepted by [`zfb_content::schema`]:
///
/// - The root value must be a JSON object.
/// - If a `"type"` key is present its value must be a recognised type name
///   string (`"string"`, `"number"`, `"integer"`, `"boolean"`, `"array"`,
///   `"object"`, `"null"`) or an array of such names.
/// - If `"type"` is `"object"` and a `"properties"` key is present, it must
///   be a JSON object (not an array, null, etc.).
///
/// Unknown keywords are accepted silently — the validator is permissive so
/// that schemas using standard JSON Schema keywords not yet understood by
/// zfb still produce a sane build rather than a hard error at config-load.
///
/// Consumers can reach the inner value via [`Deref`](std::ops::Deref) or
/// `.as_value()`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonSchema(serde_json::Value);

impl JsonSchema {
    /// Validate and wrap a [`serde_json::Value`] as a [`JsonSchema`].
    ///
    /// Returns `Err` with a human-readable message if the value is not a
    /// valid schema according to the rules described on [`JsonSchema`].
    pub fn try_from_value(v: serde_json::Value) -> Result<Self, String> {
        validate_schema_doc(&v)?;
        Ok(Self(v))
    }

    /// Borrow the inner JSON value.
    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }
}

impl std::ops::Deref for JsonSchema {
    type Target = serde_json::Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for JsonSchema {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(deserializer)?;
        JsonSchema::try_from_value(v).map_err(de::Error::custom)
    }
}

/// The accepted primitive type names in a JSON Schema `"type"` field.
const KNOWN_TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "array", "object", "null",
];

/// Validate a schema document value against the minimal dialect.
fn validate_schema_doc(v: &serde_json::Value) -> Result<(), String> {
    let obj = match v {
        serde_json::Value::Object(m) => m,
        other => {
            return Err(format!(
                "schema must be a JSON object, got {}",
                json_type_name(other)
            ));
        }
    };

    // Validate `"type"` when present.
    if let Some(type_val) = obj.get("type") {
        match type_val {
            serde_json::Value::String(s) => {
                if !KNOWN_TYPES.contains(&s.as_str()) {
                    return Err(format!(
                        "schema \"type\" value {:?} is not recognised; \
                         valid values are: {}",
                        s,
                        KNOWN_TYPES.join(", ")
                    ));
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    match item {
                        serde_json::Value::String(s) => {
                            if !KNOWN_TYPES.contains(&s.as_str()) {
                                return Err(format!(
                                    "schema \"type\" array contains unknown type {:?}; \
                                     valid values are: {}",
                                    s,
                                    KNOWN_TYPES.join(", ")
                                ));
                            }
                        }
                        other => {
                            return Err(format!(
                                "schema \"type\" array must contain strings, found {}",
                                json_type_name(other)
                            ));
                        }
                    }
                }
            }
            other => {
                return Err(format!(
                    "schema \"type\" must be a string or array of strings, got {}",
                    json_type_name(other)
                ));
            }
        }
    }

    // Validate `"properties"` when present.
    if let Some(props_val) = obj.get("properties") {
        if !props_val.is_object() {
            return Err(format!(
                "schema \"properties\" must be a JSON object, got {}",
                json_type_name(props_val)
            ));
        }
    }

    Ok(())
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// --- Config types -------------------------------------------------------------

/// Top-level `zfb.config.{ts,json}` schema.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Output directory for built assets. Default: `dist`.
    #[serde(default = "default_out_dir")]
    pub out_dir: PathBuf,

    /// Public/static directory copied verbatim. Default: `public`.
    #[serde(default = "default_public_dir")]
    pub public_dir: PathBuf,

    /// Optional dev/preview server bind host. When absent, the consuming
    /// command (`zfb dev`) falls back to its built-in default (`localhost`).
    /// Made optional so the CLI can layer "flag > config > built-in" cleanly.
    #[serde(default)]
    pub host: Option<String>,

    /// Optional dev/preview server port. When absent, the consuming command
    /// falls back to its built-in default (`3000` for `zfb dev`, `4321`
    /// for `zfb preview`). Optional for the same reason as [`host`].
    #[serde(default)]
    pub port: Option<u16>,

    /// Host header values the dev/preview server accepts when bound to
    /// a non-localhost interface (`--host 0.0.0.0`, `host` in config) —
    /// the DNS-rebinding guard from issue #931 / #919, mirroring Vite's
    /// `server.allowedHosts`.
    ///
    /// Only consulted for non-loopback binds; the default `localhost`
    /// bind skips validation entirely. `localhost`, the explicitly
    /// bound host, and any IP-literal Host — `127.0.0.1`, `[::1]`, the
    /// LAN URLs the startup banner prints — are always allowed (DNS
    /// rebinding needs a DNS name, so raw IPs are safe; Vite parity).
    ///
    /// Matching rules (the request Host's port is stripped first and
    /// comparison is case-insensitive):
    ///
    /// - `"example.com"` — matches exactly that host.
    /// - `".example.com"` (leading dot) — matches `example.com` and
    ///   every subdomain (`api.example.com`).
    /// - IPv6 entries may be written with or without brackets
    ///   (`"[::1]"` / `"::1"`).
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `allowedHosts` into this field. Mirrors
    /// `allowedHosts` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,

    /// JSX framework runtime. Default: `Preact`.
    #[serde(default)]
    pub framework: Framework,

    /// Content collections.
    #[serde(default)]
    pub collections: Vec<CollectionDef>,

    /// Tailwind-specific config; absent = default behavior.
    #[serde(default)]
    pub tailwind: Option<TailwindConfig>,

    /// Prefetch options. When `prefetch.disabled` is `true`, the bundler
    /// emits `globalThis.__zfb.prefetchDisabled = true` in `entry.mjs` and
    /// `<ClientRouter />` renders `<meta name="zfb-prefetch-disabled"
    /// content="true">` in `<head>`. The runtime's prefetch-core reads that
    /// meta tag at `init()` time and short-circuits.
    ///
    /// Absent / `None` preserves current behaviour — no prefetch meta tag is
    /// emitted and no flag is set.
    ///
    /// Mirrors `PrefetchConfig` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub prefetch: Option<PrefetchConfig>,

    /// Bundler options. `bundle.exclude` lists project-relative globs of
    /// source files the bundler must keep out of the esbuild graph (see
    /// [`BundleConfig::exclude`]). Absent / `None` → no files are skipped
    /// (byte-identical to a build without this knob).
    ///
    /// Mirrors `BundleConfig` in `packages/zfb/src/config.ts`.
    ///
    /// NOTE: This is unrelated to the `exclude` field on [`CollectionDef`]
    /// (collections / i18n locale filtering) — the two are namespaced
    /// separately so they never collide.
    #[serde(default)]
    pub bundle: Option<BundleConfig>,

    /// User-supplied plugins.
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,

    /// Deploy-target adapter package name. `None` (or omitted) means
    /// pure-static build — any route exporting `prerender = false` is
    /// rejected at build time. A package name like
    /// `"@takazudo/zfb-adapter-cloudflare"` selects the matching
    /// adapter; the build then invokes that package's bin to wrap the
    /// SSR bundle into a deploy-ready entry (e.g. `dist/_worker.js`).
    ///
    /// Accepted shapes: `None`, omitted, the literal string `"none"`,
    /// or any non-empty package name. Empty / whitespace-only strings
    /// are rejected at parse time so a typo doesn't silently fall back
    /// to no-adapter.
    ///
    /// See [`zfb_build::AdapterChoice::from_config`] for the parser
    /// the build orchestrator runs against this field.
    #[serde(default)]
    pub adapter: Option<String>,

    /// Strip `.md` / `.mdx` from internal `<a href>` paths during MDX
    /// compilation, and append a trailing `/` so the resulting URL shape
    /// converges with the rest of the site (mirrors the JS engine's
    /// `rehypeStripMdExtension`). Default: `false`.
    ///
    /// Opt-in for projects whose content authors hand-write
    /// `[label](other.md)` style references that should resolve to the
    /// rendered route URL (`other/`) rather than a literal file path.
    /// Built dist (`zfb build`) and dev rendering (`zfb dev`) both honour
    /// this flag so dev preview matches shipped output.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `stripMdExt` into this field.
    #[serde(default)]
    pub strip_md_ext: bool,

    /// Syntect code-highlight options; absent = default theme
    /// (`base16-ocean.dark`). See [`CodeHighlightConfig`] for accepted
    /// theme names and the built-in set.
    #[serde(default)]
    pub code_highlight: Option<CodeHighlightConfig>,

    /// Markdown link resolver (port of `remarkResolveMarkdownLinks`).
    ///
    /// When `Some` and `enabled: true`, the build appends
    /// [`ResolveLinksPlugin`](zfb_content::plugins::ResolveLinksPlugin) to
    /// the mdast pipeline after the directives step so author-written
    /// `[label](./other.mdx)` links rewrite to the rendered route URL.
    /// Absent / `None` / `enabled: false` preserves current pass-through.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON/TS form `resolveMarkdownLinks` into this field.
    #[serde(default)]
    pub resolve_markdown_links: Option<ResolveMarkdownLinksConfig>,

    /// Public URL prefix mounted in front of every absolute HTML asset
    /// URL the build emits (`<link rel="stylesheet">`,
    /// `<script type="module">`, and any other `/assets/...`-prefixed
    /// reference the production asset pipeline rewrites).
    ///
    /// Use this when the site is deployed under a sub-path
    /// (`https://example.com/pj/zudo-doc/`). With `base = "/pj/zudo-doc/"`
    /// the dist HTML emits
    /// `<link rel="stylesheet" href="/pj/zudo-doc/assets/styles-<hash>.css">`
    /// instead of the unprefixed `/assets/styles-<hash>.css`.
    ///
    /// Accepted shapes:
    ///
    /// - `None`, omitted, `""`, `"/"` — no prefix; behaviour matches
    ///   the pre-`base` build byte-for-byte.
    /// - leading-and-trailing-slash path like `"/pj/zudo-doc/"`.
    /// - absolute URL like `"https://cdn.example.com/"`.
    ///
    /// Normalisation lives at the asset-URL emission boundary
    /// ([`asset_url_base_prefix`]) — the field stores the value as
    /// authored. `#[serde(rename_all = "camelCase")]` on this struct
    /// deserialises the JSON / TS form `base` 1:1.
    #[serde(default)]
    pub base: Option<String>,

    /// Whether the basePath rewriter should append a trailing `/` to
    /// extensionless absolute hrefs (`<a href="/docs/foo">` becomes
    /// `<a href="/pj/zudo-doc/docs/foo/">` when `base = "/pj/zudo-doc/"`
    /// and this is `true`).
    ///
    /// Off by default — preserves byte-for-byte parity with the
    /// pre-`trailing_slash` build for projects that haven't opted in.
    /// Enable when the deploy target serves canonical URLs with
    /// trailing slashes (Cloudflare Pages with `trailingSlash: always`,
    /// Netlify pretty URLs, etc.) so the dist HTML doesn't ship
    /// non-canonical hrefs that 301-redirect on every click.
    ///
    /// Only the trailing slash for extensionless hrefs is affected.
    /// Hrefs that already end in `/`, that have a file extension
    /// (`.png`, `.pdf`, …), or that opt out via `data-no-base` pass
    /// through unchanged.
    #[serde(default)]
    pub trailing_slash: bool,

    /// Markdown / MDX parsing options. Currently the only knob exposed
    /// is [`MarkdownConfig::gfm`], which toggles GFM constructs
    /// (strikethrough, table, autolink-literal, task-list-item,
    /// footnote-definition) on or off.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `markdown` into this field.
    #[serde(default)]
    pub markdown: Option<MarkdownConfig>,

    /// Whether `zfb build` writes the post-build route manifest to disk
    /// at `<outDir>/__zfb/routes.json` (#347).
    ///
    /// The on-disk file mirrors the in-memory `ctx.routes` shape exposed
    /// to `postBuild` plugins — same fields, same sort order — so any
    /// build-pipeline consumer (a `pnpm build && custom-script` script,
    /// a sibling generator, etc.) can read the manifest without writing
    /// a zfb plugin. The two surfaces are two access shapes over the
    /// same data, not two independent contracts.
    ///
    /// Default: emit (`None` is treated as `true`). Pass `false` to skip
    /// the write — useful for projects that strip everything but
    /// shipped assets out of `dist/` before deploy.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `emitRoutesManifest` into this field.
    #[serde(default)]
    pub emit_routes_manifest: Option<bool>,

    /// Canonical origin URL for the site (e.g. `"https://example.com"`).
    ///
    /// When set, the bundler emits `globalThis.__zfb.site = <value>` in
    /// the synthetic `entry.mjs` so layouts can build canonical `<link>`
    /// tags, OpenGraph `og:url` meta, sitemap absolute hrefs, and
    /// hreflang `<link rel="alternate">` from a single config-level
    /// source of truth.
    ///
    /// Accepted shape: an absolute HTTP or HTTPS URL. Relative URLs,
    /// non-HTTP(S) schemes (e.g. `ftp://`), and empty strings are
    /// rejected at config-load time with a clear error message. The
    /// value is stored verbatim (trailing slash normalisation is the
    /// consumer's responsibility).
    ///
    /// When absent, `globalThis.__zfb.site` is not emitted — the build
    /// output is byte-for-byte identical to the pre-`site` build.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `site` 1:1.
    #[serde(default)]
    pub site: Option<String>,

    /// Extra absolute filesystem paths watched by the dev server in
    /// addition to the project-root tree.
    ///
    /// Use this when project content sources its data from outside the
    /// project root (a sibling knowledge-base repo, a shared filesystem
    /// directory, a `file:` dep that ships content alongside code, etc.)
    /// and you want `zfb dev` to live-reload when those external files
    /// change.
    ///
    /// **Semantics (validated at config-load + applied by the dev
    /// command):**
    ///
    /// - Each entry MUST be an absolute path. Relative paths are
    ///   rejected at config-load with a clear error message.
    /// - Each entry is canonicalised (`Path::canonicalize`) when the
    ///   watcher boots — events match the canonical form.
    /// - A path that does NOT exist at boot is skipped with a
    ///   warning; the watcher does NOT re-watch the path if it
    ///   appears later. Restart `zfb dev` after creating the path.
    /// - Each entry is watched recursively.
    /// - Events from outside the project root bypass fine-grained
    ///   graph classification and may trigger a broader rebuild
    ///   than equivalent in-tree edits (the dependency graph only
    ///   tracks in-tree edges).
    ///
    /// **Security note:** opt-in only — do NOT point this at unbounded
    /// directories like `$HOME` or `/`. The recursive watch will try to
    /// register every subdirectory and (on Linux) hit the inotify
    /// `max_user_watches` ceiling on large trees.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `extraWatchPaths` into this field.
    #[serde(default)]
    pub extra_watch_paths: Vec<PathBuf>,

    /// Project output mode. Drives the V8-mode decision the build
    /// engine makes at the detection seam (sub-task 4.1b / issue
    /// #373) — see [`OutputMode`] for the decision tree.
    ///
    /// Default: [`OutputMode::Auto`] — detection-driven (non-empty
    /// `prerender = false` route set => V8-on; empty => V8-off).
    /// Explicit `"static"` and `"hybrid"` are the manual overrides.
    ///
    /// **Today's load-bearing role** is the precondition check —
    /// `output: "static"` + detected SSR routes is a hard build error.
    /// The V8-off branch does NOT skip V8 host startup yet; the
    /// shipping `zfb` binary still needs V8 to render SSG pages. The
    /// flag exists as infrastructure for the future shipping path
    /// (Tauri sidecar / standalone SSR server) where a V8-less Rust
    /// runtime would be possible — see
    /// `research/344-v8-feature-gate.md` for the rationale.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `output` 1:1.
    #[serde(default)]
    pub output: OutputMode,

    /// Maximum seconds a single plugin lifecycle hook (preBuild,
    /// postBuild, setup, etc.) may run before the build fails with a
    /// diagnostic error and the plugin host is force-killed.
    ///
    /// Absent / `None` falls through to the `ZFB_PLUGIN_HOOK_TIMEOUT`
    /// env var, then the 120s built-in default. Set this when your
    /// plugins do long but bounded work (e.g. large sitemap generation)
    /// and you want a tighter or more explicit budget. Seconds.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `pluginHookTimeoutSecs` 1:1.
    #[serde(default)]
    pub plugin_hook_timeout_secs: Option<u64>,

    /// Whether `copy_public_dir` copies `public/` under the `base`
    /// sub-path segment (`true`, default) or flat to the `dist/` root
    /// (`false`).
    ///
    /// **`true` (default):** files land at
    /// `<outDir>/<base-segment>/<rel>`, matching the base-prefixed URLs
    /// that `withBase()` emits in the rendered HTML. This is the
    /// canonical placement for projects served at their configured sub-
    /// path — a file at `public/img/logo.svg` is reachable at
    /// `/<base>/img/logo.svg` in production.
    ///
    /// **`false`:** files land flat at `<outDir>/<rel>` regardless of
    /// `base`. Use this when the deploy pipeline relocates the entire
    /// `dist/` tree into the base segment itself (e.g. `cp -a dist/.
    /// deploy-root/pj/site/`), so putting the files under
    /// `<outDir>/<base>/...` would result in a double-nested path. In
    /// that scheme `public/img/logo.svg` lands at `dist/img/logo.svg`
    /// and arrives at `/<base>/img/logo.svg` after relocation — the same
    /// final URL, without the redundant nesting.
    ///
    /// **Interaction with `zfb preview`:** with `false`, base-prefixed
    /// asset URLs 404 under `zfb preview` because the flat copy lives at
    /// the dist root. This is a known trade-off of the flat-copy deploy
    /// scheme; `zfb preview` does not simulate deploy-side relocation.
    ///
    /// `#[serde(rename_all = "camelCase")]` on this struct deserialises
    /// the JSON / TS form `copyPublicWithBase` 1:1.
    #[serde(default = "default_true")]
    pub copy_public_with_base: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            out_dir: default_out_dir(),
            public_dir: default_public_dir(),
            host: None,
            port: None,
            allowed_hosts: Vec::new(),
            framework: Framework::default(),
            collections: Vec::new(),
            tailwind: None,
            prefetch: None,
            bundle: None,
            plugins: Vec::new(),
            adapter: None,
            strip_md_ext: false,
            base: None,
            code_highlight: None,
            resolve_markdown_links: None,
            trailing_slash: false,
            markdown: None,
            site: None,
            emit_routes_manifest: None,
            extra_watch_paths: Vec::new(),
            output: OutputMode::default(),
            plugin_hook_timeout_secs: None,
            copy_public_with_base: true,
        }
    }
}

/// Project output mode (`zfb.config.ts` field `output`).
///
/// Drives the V8-mode decision the build engine makes right after the
/// no-SSR-without-adapter precondition check (see
/// `crates/zfb/src/commands/build.rs`, function
/// [`resolve_v8_mode`](crate::commands::build::resolve_v8_mode)).
///
/// Decision tree:
///
/// - [`OutputMode::Static`] — declare a pure-static (SSG-only) project.
///   Errors at build start if any route exports `prerender = false`,
///   pointing at the offending route so the user can either remove the
///   `prerender = false` or switch to `output: "hybrid"`.
/// - [`OutputMode::Hybrid`] — declare a project that may host SSR
///   routes. V8-on regardless of detection, even when no `prerender =
///   false` route currently exists. Useful for projects that will add
///   SSR routes later and want a stable build topology in the
///   meantime.
/// - [`OutputMode::Auto`] — detection-driven (the v1 default). The
///   build inspects the `prerender = false` route set: non-empty
///   => V8-on, empty => V8-off.
///
/// See [`Config::output`] for the field-level docs and
/// `research/344-v8-feature-gate.md` for the design rationale.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Pure-static (SSG-only). Build errors if SSR routes are present.
    Static,
    /// May host SSR routes. V8-on regardless of route detection.
    Hybrid,
    /// Detection-driven (default).
    #[default]
    Auto,
}

/// JSX runtime selection. `Preact` is the v1 default.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    #[default]
    Preact,
    React,
}

/// One content collection (e.g. blog posts under `content/blog/`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollectionDef {
    /// Identifier used at the call site (e.g. `"blog"`).
    pub name: String,
    /// Directory (relative to the project root) holding the entries.
    pub path: PathBuf,
    /// Optional JSON Schema describing the frontmatter shape for entries in
    /// this collection. Validated at config-load time; malformed schemas
    /// (unknown `"type"` values, non-object `"properties"`, etc.) are
    /// rejected before the build runs. See [`JsonSchema`] for the accepted
    /// dialect.
    #[serde(default)]
    pub schema: Option<JsonSchema>,
    /// Optional glob patterns (evaluated relative to `path`). When set
    /// and non-empty, an entry is kept only if at least one pattern
    /// matches its path relative to `path`. When `None` or empty, no
    /// include-filtering happens (every candidate file is kept).
    ///
    /// Mirrors Astro's content-collection `glob({ pattern })` include
    /// shape. Patterns use the `globset` dialect (Unix-style globs:
    /// `*`, `**`, `?`, `[…]`).
    #[serde(default)]
    pub include: Option<Vec<String>>,
    /// Optional glob patterns (evaluated relative to `path`). When set
    /// and non-empty, an entry is dropped if any pattern matches its
    /// path relative to `path`. Evaluated AFTER `include` — the
    /// effective set is `(include ∪ all) ∩ ¬exclude`.
    ///
    /// Mirrors Astro's `['**/*.mdx', '!**/*.en.mdx']` pattern. zfb
    /// splits the negative side into its own field.
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    /// Optional suffix to strip from each kept entry's slug. When the
    /// slug (filename minus extension) ends with the given suffix, the
    /// suffix is stripped from both `Entry::slug` and
    /// `Entry::module_specifier`. Other entries are unchanged.
    ///
    /// Example: with `idStripSuffix: ".en"`, `col003-mixers.en.mdx` ->
    /// slug `col003-mixers`, specifier
    /// `mdx://notes-en/col003-mixers#<hash>`. Consumer code calls
    /// `getEntry('notes-en', 'col003-mixers')` without knowing about
    /// the suffix.
    ///
    /// Useful for multi-locale layouts where one source directory
    /// holds both `foo.mdx` (default locale) and `foo.en.mdx` (locale
    /// override). Pair with `include` / `exclude` to route each locale
    /// into its own collection.
    #[serde(default)]
    pub id_strip_suffix: Option<String>,
}

/// Tailwind options. Empty by default (Tailwind enabled); users can flip
/// `enabled: false` to opt out.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TailwindConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Prefetch options. Mirrors `PrefetchConfig` in `packages/zfb/src/config.ts`.
///
/// When `disabled` is `Some(true)`, the bundler emits
/// `globalThis.__zfb.prefetchDisabled = true` in `entry.mjs`, and
/// `<ClientRouter />` renders `<meta name="zfb-prefetch-disabled"
/// content="true">` in `<head>`. The runtime's prefetch-core reads that meta
/// tag at `init()` time and short-circuits so no prefetch wiring runs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrefetchConfig {
    /// Disable prefetch entirely. Default: `None` (equivalent to `false`).
    #[serde(default)]
    pub disabled: Option<bool>,
}

/// Bundler options (`zfb.config.ts` field `bundle`).
///
/// Mirrors `BundleConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BundleConfig {
    /// Project-relative glob patterns (gitignore-style) for source files
    /// the bundler must NOT pull into the esbuild graph.
    ///
    /// Each pattern is matched against the file's path relative to the
    /// project root, in POSIX form (e.g. `components/Foo.stories.tsx` or
    /// `components/**/*.stories.tsx`). A matched file is never
    /// copied/symlinked into the shadow tree AND is dropped from any eager
    /// `import.meta.glob(...)` expansion that would otherwise statically
    /// import it.
    ///
    /// Why this exists: a `--platform=neutral` worker bundle rejects
    /// CJS-only packages that resolve only via `main`/`module` or a
    /// `require`-only `exports` condition (e.g. `msw` → `path-to-regexp@6`).
    /// An eager glob over `components/**/*.stories.tsx` newly pulls such a
    /// package in; excluding the offending file keeps the build green.
    ///
    /// Absent / empty → no files are skipped (byte-identical to a build
    /// without this knob).
    ///
    /// NOTE: This is unrelated to the `exclude` field on [`CollectionDef`]
    /// (collections / i18n locale filtering).
    #[serde(default)]
    pub exclude: Option<Vec<String>>,

    /// Explicit esbuild `main-fields` list for the `--platform=neutral`
    /// page/SSR pass. Under `neutral` esbuild's main-fields list is EMPTY by
    /// default, so a dep resolved purely via `package.json` `main`/`module`
    /// (no `exports` map) fails with `The "main" field here was ignored. Main
    /// fields must be configured explicitly when using the "neutral"
    /// platform.` Setting e.g. `["main", "module"]` lets such CJS-main-only
    /// deps resolve (#676 -- `msw` -> `path-to-regexp@6`). Applies to every
    /// framework; absent/empty -> byte-identical to a build without the knob
    /// (the React-only `main,module` shim still applies).
    ///
    /// Mirrors `BundleConfig.mainFields` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub main_fields: Option<Vec<String>>,

    /// Bare specifiers to mark `--external` in the `--platform=neutral`
    /// page/SSR pass, so esbuild leaves them unbundled instead of resolving
    /// them (the other #676 escape hatch -- externalize a CJS-only dep rather
    /// than resolving it). Appended to the framework-provided externals.
    /// Absent/empty -> no extra externals.
    ///
    /// Mirrors `BundleConfig.external` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub external: Option<Vec<String>>,
}

/// Syntect-based code-highlight options.
///
/// Controls the built-in syntax-highlight theme applied to fenced code
/// blocks in MDX content. Theme names are syntect's built-in set:
/// `"base16-ocean.dark"` (default), `"base16-ocean.light"`,
/// `"InspiredGitHub"`, `"Solarized (dark)"`, `"Solarized (light)"`.
///
/// To use a custom theme (e.g. Dracula), drop the `.tmTheme` file into
/// a directory and set `themesDir` to point at it.  The theme name to
/// pass in `theme` is the name declared inside the `.tmTheme` plist
/// (its `name` key).
///
/// **Note:** These are NOT Shiki theme names.
///
/// Unknown theme names are rejected with a clear error rather than
/// silently falling back — this matches the behaviour of
/// [`zfb_content::syntect_highlight::Highlighter::set_default_theme`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeHighlightConfig {
    /// Syntect built-in or user-loaded theme name.  When absent the
    /// pipeline defaults to `"base16-ocean.dark"`.
    ///
    /// Mutually exclusive with [`Self::theme_light`] / [`Self::theme_dark`]
    /// — set only one mode per build.  This is a SYNTECT theme name (e.g.
    /// `"InspiredGitHub"`, `"Solarized (dark)"`), NOT a Shiki name like
    /// `"dracula"`.
    #[serde(default)]
    pub theme: Option<String>,

    /// Path to a directory of `.tmTheme` files, relative to the
    /// project root.  Every `.tmTheme` file in the directory is loaded
    /// and becomes available by its declared `name` via `theme`,
    /// `theme_light`, or `theme_dark`.
    ///
    /// When absent only syntect's bundled themes are available.
    ///
    /// The path must be relative and must not escape the project root
    /// via `..`.  A missing directory is reported as an error at build
    /// start (before any pages are rendered).
    ///
    /// Applies to both single-theme and dual-theme mode.
    #[serde(default)]
    pub themes_dir: Option<std::path::PathBuf>,

    /// Light-mode syntect theme name for dual-theme highlighting.
    ///
    /// Must be set together with [`Self::theme_dark`] — setting only one
    /// of the two is an error.  When both are set, every fenced code block
    /// is highlighted twice and per-token colors are emitted as CSS custom
    /// properties (`--shiki-light` / `--shiki-dark`) instead of inline
    /// `color:`.  The `<pre>` element carries `class="syntect-dual"` and
    /// `--shiki-*-bg` variables.  The consumer resolves the active color
    /// with a `light-dark()` CSS rule.
    ///
    /// Mutually exclusive with [`Self::theme`].
    /// Must be a SYNTECT theme name (e.g. `"base16-ocean.light"`), NOT a
    /// Shiki name like `"dracula"`.
    #[serde(default)]
    pub theme_light: Option<String>,

    /// Dark-mode syntect theme name for dual-theme highlighting.
    ///
    /// See [`Self::theme_light`] for the full dual-mode contract.
    /// Must be set together with [`Self::theme_light`] — setting only one
    /// of the two is an error.  Must be a SYNTECT theme name (e.g.
    /// `"base16-ocean.dark"`), NOT a Shiki name.
    ///
    /// Mutually exclusive with [`Self::theme`].
    #[serde(default)]
    pub theme_dark: Option<String>,
}

/// What to do when a `.md`/`.mdx` link cannot be found in the source map.
///
/// Mirrors the JS engine's `onBrokenLinks` option:
/// - `"warn"` — emit a warning to stderr but continue the build.
/// - `"error"` — accumulate all broken links then return an error after
///   the walk completes (so every broken link is reported in one pass).
/// - `"ignore"` — silently ignore broken links (no warnings, no errors).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum OnBrokenLinks {
    /// Emit a warning to stderr but continue. This is the default.
    #[default]
    Warn,
    /// Return an error after the walk completes (all broken links reported).
    Error,
    /// Silently ignore broken links.
    Ignore,
}

/// Config for the `ResolveLinksPlugin` (port of `remarkResolveMarkdownLinks`).
///
/// When `enabled` is `true`, the build appends `ResolveLinksPlugin` to the
/// mdast pipeline after the directives step so author-written
/// `[label](./other.mdx)` links are rewritten to the corresponding rendered
/// route URL (e.g. `/docs/other/`).
///
/// Two ways to specify the source dirs:
///
/// - **Single dir (legacy):** set `docs_dir` and the build assumes the
///   `/docs/` route prefix. Convenient for single-locale projects.
/// - **Multi dir (`dirs` non-empty):** explicit list of `{ dir, route_prefix }`
///   entries — required for any project with locale mirrors (e.g. `docs/`
///   AND `docs-ja/`) so each dir maps to its own route prefix
///   (`/docs/` vs `/ja/docs/`). When `dirs` is non-empty, `docs_dir` is
///   ignored and only `dirs` is consulted.
///
/// Default (absent / `enabled: false`) preserves the current pass-through
/// behavior — links are not rewritten.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMarkdownLinksConfig {
    /// Whether to enable link resolution. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Legacy single-dir field. Used only when [`Self::dirs`] is empty.
    /// When non-empty, scanned against the hard-coded `/docs/` route
    /// prefix. Interpreted relative to the project root the same way
    /// `CollectionDef::path` is — must be relative and must not escape
    /// the root via `..`.
    #[serde(default)]
    pub docs_dir: PathBuf,
    /// Explicit per-dir source map. Each entry is one collection
    /// (e.g. EN docs at `src/content/docs/` → `/docs/`, JA docs at
    /// `src/content/docs-ja/` → `/ja/docs/`). Takes precedence over
    /// [`Self::docs_dir`] when non-empty.
    ///
    /// Required for any project with more than one docs root because
    /// the legacy `docs_dir` shape only supports a single hard-coded
    /// `/docs/` route prefix and cannot represent locale mirrors that
    /// must resolve under `/{locale}/docs/`. See
    /// `zudolab/zudo-doc#1577` for the host-side bug this surfaces.
    #[serde(default)]
    pub dirs: Vec<ResolveMarkdownLinksDir>,
    /// What to do when a `.md`/`.mdx` link cannot be resolved. Default: `"warn"`.
    #[serde(default)]
    pub on_broken_links: OnBrokenLinks,
}

/// One source dir entry for [`ResolveMarkdownLinksConfig::dirs`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMarkdownLinksDir {
    /// Directory (relative to project root) whose `.md`/`.mdx` files
    /// are scanned. Must be relative and must not escape the root via
    /// `..` — validated at config load.
    pub dir: PathBuf,
    /// Route prefix prepended to each file's slug. Include leading and
    /// trailing slashes (e.g. `"/docs/"` or `"/ja/docs/"`).
    pub route_prefix: String,
}

impl Default for TailwindConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// One user plugin entry.
///
/// `name` is the user-supplied reference written in `zfb.config.ts`
/// (npm package name or `./`-relative module path). `resolved_module`
/// is the absolute module specifier (a `file://` URL) the JS-side
/// plugin host will hand to `import()`. It is `None` for the JSON-
/// config path and for synthetic configs constructed in tests; the
/// [`Config`] consumers that actually load plugins (the build/dev
/// orchestration) treat a `None` `resolved_module` as "no plugin
/// module — skip this entry".
///
/// Sub 3 / issue #108: on the TS load path the evaluator (in-process V8
/// on default builds, `crates/zfb/js/config-loader.mjs` subprocess on
/// slim builds) emits a `{ config, plugins }` envelope where `plugins[i]`
/// is the absolute module specifier for `config.plugins[i].name`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default)]
    pub options: serde_json::Value,
    /// Absolute module specifier (file URL) the plugin host will load
    /// via dynamic `import()`. Populated by the TS-load path; `None`
    /// for JSON-only configs and synthetic test configs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_module: Option<String>,
}

// --- markdown / GFM config -------------------------------------------------

// TocConfig now lives in `zfb-md-ast::features_config` (re-exported below
// alongside the other feature config types). `zfb-content` still
// re-exports it under the historical `zfb_content::TocConfig` path for
// downstream code that imports through that crate's lib surface.

/// Markdown / MDX parsing options.
///
/// Mirrors `MarkdownConfig` in `packages/zfb/src/config.ts`. Knobs include
/// [`Self::gfm`], [`Self::toc`], [`Self::external_links`],
/// [`Self::cjk_friendly`], and [`Self::features`]; future markdown-pipeline
/// knobs would also live here.
///
/// See the "Markdown Features" docs category for the per-feature option
/// reference once individual features are ported.
///
/// `#[serde(rename_all = "camelCase")]` on this struct (and on the
/// parent [`Config`]) makes the TS shape (`{ gfm: ... }`) round-trip 1:1
/// with the Rust shape.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownConfig {
    /// Enable GFM constructs. Accepts three shapes — see [`GfmFlag`].
    /// Absent / `None` defers to the conservative resolved default in
    /// [`MarkdownConfig::resolve_constructs`] (strikethrough + table on,
    /// other GFM constructs off).
    #[serde(default)]
    pub gfm: Option<GfmFlag>,

    /// Table-of-contents options. When `Some`, a [`TocPlugin`] is wired
    /// into the hast phase (after `HeadingLinksPlugin`) that inserts a
    /// `<ul>/<li>` TOC list after the first heading whose text matches
    /// `heading`. When `None` (the default), the visitor is not registered
    /// and the build is byte-for-byte identical to the pre-TOC build.
    ///
    /// `#[serde(rename_all = "camelCase")]` on [`TocConfig`] deserialises
    /// the TS shape `{ heading?: string; maxDepth?: number }`.
    #[serde(default)]
    pub toc: Option<TocConfig>,
    /// External-link rewriter. When `Some`, every `<a>` whose href is
    /// classified as external gains `target` and `rel` attributes.
    /// `None` (absent) = feature disabled; output byte-identical to today.
    ///
    /// Mirrors `markdown.externalLinks` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub external_links: Option<ExternalLinksConfig>,

    /// Enable CJK-friendly markdown handling.
    ///
    /// Governs two mdast post-processors that adapt CommonMark/GFM rules to
    /// CJK text: [`CjkFriendlyPlugin`] (emphasis/strong flanking around CJK
    /// punctuation) and [`CjkAutolinkBoundaryPlugin`] (terminating a GFM
    /// autolink-literal path at the first CJK character — zfb#1105; only
    /// active when `gfm.autolinkLiteral` is also on).
    ///
    /// - `None` (absent, default) — CJK-friendly is **on**. Preserves
    ///   today's behaviour so existing CJK-content sites are unaffected
    ///   by the new field.
    /// - `Some(true)` — explicit opt-in; identical to absent.
    /// - `Some(false)` — opt-out. Neither plugin is added to the mdast
    ///   pipeline; emphasis markers and bare-URL autolinks adjacent to CJK
    ///   characters follow base CommonMark/GFM rules (rarely the right
    ///   choice; provided as an escape hatch for projects that need strict
    ///   CommonMark/GFM output).
    ///
    /// [`CjkFriendlyPlugin`]: zfb_content::plugins::CjkFriendlyPlugin
    /// [`CjkAutolinkBoundaryPlugin`]: zfb_content::plugins::CjkAutolinkBoundaryPlugin
    #[serde(default)]
    pub cjk_friendly: Option<bool>,

    /// Convert every soft line break (a single `\n` inside a paragraph)
    /// into `<br>` (remark-breaks parity).
    ///
    /// - `None` (absent, default) — soft line breaks are collapsed into a
    ///   single space, following standard CommonMark behaviour.
    /// - `Some(true)` — opt-in; every `\n` inside a paragraph becomes `<br>`.
    /// - `Some(false)` — explicit opt-out; identical to absent.
    ///
    /// Mirrors `hardBreaks` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub hard_breaks: Option<bool>,

    /// Per-feature markdown pipeline toggles. Each field is a
    /// [`FeatureToggle`] (`true` / `false` / per-feature options object)
    /// or a feature-specific config struct (for features that require
    /// extra parameters, e.g. `githubAutolinks`).
    ///
    /// Absent / `None` means all features are disabled. As of the
    /// v0.1.0-next.12 epic (#583, wired in #586) this includes the
    /// former-Core framework features (`mermaid`, `directives`,
    /// `headingMarkerToc`), which are now OFF by default and must be opted into
    /// via this object — so the default build is NOT byte-identical to the
    /// pre-epic always-on behaviour.
    ///
    /// Unknown keys in the `features` object are rejected with a clear
    /// deserialization error naming the unknown field.
    #[serde(default)]
    pub features: Option<MarkdownFeaturesConfig>,
}

// --- MarkdownFeaturesConfig and per-feature types --------------------------
//
// Canonical definitions live in `zfb-md-ast::features_config` so the
// visitor-contract crate, `zfb-md-extras`, AND this user-facing `zfb`
// crate all share a single source of truth — no parallel definitions,
// no conversion bridges. Re-exported here so existing import paths
// like `zfb::config::{MarkdownFeaturesConfig, FeatureToggle, ...}` and
// `zfb::config::TocConfig` continue to resolve.
pub use zfb_md_ast::{
    directives_enabled, heading_id_strategy, into_directive_def, reading_time_enabled,
    CodeEnrichmentConfig, DirectiveFullSpec, DirectiveSpec, DirectiveSpecKind, FeatureOptions,
    FeatureToggle, GithubAutolinksConfig, HeadingIdStrategy, HeadingIdsConfig,
    HeadingMarkerTocFeature, ImageDimensionsConfig, LinkValidationConfig, MarkdownFeaturesConfig,
    ReadingTimeFeature, ReadingTimeOptions, TocConfig, TocExportConfig, TranscludeConfig,
};

/// Options for the `rehype-external-links` port.
///
/// An absent field uses the documented default so the shape is additive:
/// existing configs that add `externalLinks: {}` get the safe defaults
/// without spelling out every field.
///
/// Mirrors `ExternalLinksConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLinksConfig {
    /// `rel` tokens applied to external links.
    /// Default: `["noopener", "noreferrer"]`.
    #[serde(default)]
    pub rel: Option<Vec<String>>,
    /// `target` value for external links.
    /// Default: `"_blank"`.
    #[serde(default)]
    pub target: Option<String>,
}

impl ExternalLinksConfig {
    /// Convert to the `zfb_content` plugin config, applying documented
    /// defaults for absent fields.
    #[must_use]
    pub fn into_content_config(self) -> zfb_content::ExternalLinksConfig {
        zfb_content::ExternalLinksConfig {
            rel: self
                .rel
                .unwrap_or_else(|| vec!["noopener".to_string(), "noreferrer".to_string()]),
            target: self.target.unwrap_or_else(|| "_blank".to_string()),
        }
    }
}

/// Either the shorthand boolean form (`true` = all GFM constructs on,
/// `false` = all off) or a partial [`GfmConstructs`] object that toggles
/// individual constructs.
///
/// `serde(untagged)` plus the `Constructs` variant being listed FIRST
/// is important: serde tries variants in order, so the object form
/// must be tried before the boolean form. (`true` / `false` will never
/// deserialise into the object variant anyway.)
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum GfmFlag {
    /// Per-construct opt-in / opt-out.
    Constructs(GfmConstructs),
    /// Shorthand: `true` = every GFM construct on, `false` = every GFM
    /// construct off.
    All(bool),
}

/// Per-construct opt-in / opt-out for GFM. Every field is optional;
/// omitted fields fall back to the conservative default
/// (`strikethrough: Some(true)`, `table: Some(true)`, others
/// `Some(false)` at resolve time — see
/// [`MarkdownConfig::resolve_constructs`]).
///
/// Wrapping each field in `Option<bool>` (rather than plain `bool`)
/// lets `resolve_constructs` distinguish "user explicitly said `false`"
/// from "user did not mention this field" — the latter falls back to
/// the conservative default; the former is honoured verbatim.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GfmConstructs {
    /// GFM strikethrough (`~~text~~` → `<del>text</del>`).
    #[serde(default)]
    pub strikethrough: Option<bool>,
    /// GFM pipe-style tables.
    #[serde(default)]
    pub table: Option<bool>,
    /// GFM autolink literal — bare URLs like `https://example.com`
    /// become clickable without `<…>` brackets.
    #[serde(default)]
    pub autolink_literal: Option<bool>,
    /// GFM task list items (`- [x]` / `- [ ]`).
    #[serde(default)]
    pub task_list_item: Option<bool>,
    /// GFM footnote definitions (`[^ref]: …`).
    #[serde(default)]
    pub footnote_definition: Option<bool>,
}

/// Resolved per-construct GFM flags.
///
/// Re-exported from `zfb_content::ResolvedGfmConstructs` — the type
/// itself lives in `zfb-content` (the lowest crate that actually
/// touches `markdown::Constructs`) so the snapshot walker, bundler,
/// and dev loader can wire it into [`markdown::ParseOptions`] without
/// an upward dependency on this crate. This re-export keeps the public
/// `zfb::config` surface self-contained for `zfb.config.ts` consumers.
pub use zfb_content::ResolvedGfmConstructs;

impl MarkdownConfig {
    /// Pure function — overlay this config's `gfm` field onto a
    /// [`ResolvedGfmConstructs`] base.
    ///
    /// Resolution rules:
    ///
    /// - `gfm: None` (absent) → return the conservative default
    ///   ([`ResolvedGfmConstructs::CONSERVATIVE`]). The `base` argument
    ///   is provided so callers that want a different "what fields the
    ///   user did NOT mention fall back to" can supply it; the default
    ///   pipeline path always passes [`ResolvedGfmConstructs::CONSERVATIVE`].
    /// - `gfm: Some(All(true))` → [`ResolvedGfmConstructs::ALL_ON`].
    /// - `gfm: Some(All(false))` → [`ResolvedGfmConstructs::ALL_OFF`].
    /// - `gfm: Some(Constructs(partial))` → overlay each `Some(_)`
    ///   field of `partial` onto `base`; `None` fields keep the `base`
    ///   value. So `{ strikethrough: true }` only flips strikethrough
    ///   relative to the conservative default; the other four
    ///   constructs keep their default values.
    ///
    /// Covered by the four-case test matrix in this module's `#[cfg(test)]`
    /// block.
    #[must_use]
    pub fn resolve_constructs(&self, base: ResolvedGfmConstructs) -> ResolvedGfmConstructs {
        match &self.gfm {
            None => base,
            Some(GfmFlag::All(true)) => ResolvedGfmConstructs::ALL_ON,
            Some(GfmFlag::All(false)) => ResolvedGfmConstructs::ALL_OFF,
            Some(GfmFlag::Constructs(c)) => ResolvedGfmConstructs {
                strikethrough: c.strikethrough.unwrap_or(base.strikethrough),
                table: c.table.unwrap_or(base.table),
                autolink_literal: c.autolink_literal.unwrap_or(base.autolink_literal),
                task_list_item: c.task_list_item.unwrap_or(base.task_list_item),
                footnote_definition: c.footnote_definition.unwrap_or(base.footnote_definition),
            },
        }
    }
}

/// Convenience helper: resolve `cfg.markdown.as_ref()` — handling both
/// "the user omitted the whole `markdown` block" and "the user wrote
/// `markdown: { gfm: …}`" — to a final [`ResolvedGfmConstructs`].
///
/// Pure, deterministic, zero-allocation; intended to be called from the
/// bundler / loader / snapshot-bridge sites that need the resolved
/// flags. Matching the conservative-default rule everywhere (and only
/// here) is what keeps snapshot ↔ bundler hashes byte-identical — the
/// content_bridge land mine in
/// `crates/zfb-content/src/content_bridge.rs:118-153`.
#[must_use]
pub fn resolve_gfm_constructs(markdown: Option<&MarkdownConfig>) -> ResolvedGfmConstructs {
    match markdown {
        Some(m) => m.resolve_constructs(ResolvedGfmConstructs::CONSERVATIVE),
        None => ResolvedGfmConstructs::CONSERVATIVE,
    }
}

/// Convenience helper: resolve `cfg.markdown.as_ref()` — handling both
/// "the user omitted the whole `markdown` block" and "the user wrote
/// `markdown: { cjkFriendly: false }`" — to a final `bool`.
///
/// Returns `true` (plugin on) when the field is absent or `Some(true)`;
/// `false` only when `cjk_friendly: Some(false)`.
///
/// Must be kept in sync with the bundler and snapshot walker so the
/// `CjkFriendlyPlugin` is either present in BOTH or absent in BOTH —
/// exactly as `resolve_gfm_constructs` guards the GFM construct set.
#[must_use]
pub fn resolve_cjk_friendly(markdown: Option<&MarkdownConfig>) -> bool {
    match markdown {
        Some(m) => m.cjk_friendly.unwrap_or(true),
        None => true,
    }
}

/// Convenience helper: resolve the final `hard_breaks` bool from an optional
/// `MarkdownConfig`. Returns `false` (plugin off) when the field is absent or
/// `Some(false)`; `true` only when `hard_breaks: Some(true)`.
///
/// Default is `false` — the opposite of `resolve_cjk_friendly` (which
/// defaults to `true`). Do NOT copy `unwrap_or(true)` from that function
/// here; a true on one side + false on the other diverges the
/// `content_hash` and causes silent `<pre data-zfb-content-fallback>`.
#[must_use]
pub fn resolve_hard_breaks(markdown: Option<&MarkdownConfig>) -> bool {
    match markdown {
        Some(m) => m.hard_breaks.unwrap_or(false),
        None => false,
    }
}

/// Convenience helper: resolve the final `bundle.exclude` glob list from an
/// optional [`BundleConfig`]. Returns an empty vec when `bundle` is absent or
/// `bundle.exclude` is `None`, so callers can thread the result straight into
/// `BundlerInput::bundle_exclude` and an empty vec means "skip nothing"
/// (byte-identical to a build without the knob).
#[must_use]
pub fn resolve_bundle_exclude(bundle: Option<&BundleConfig>) -> Vec<String> {
    match bundle {
        Some(b) => b.exclude.clone().unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Resolve `bundle.mainFields` into the esbuild `--main-fields` list for the
/// page/SSR pass. Empty when `bundle` is absent or `bundle.mainFields` is
/// `None`, so callers can thread the result straight into
/// `BundlerInput::main_fields` and an empty vec means "emit no
/// `--main-fields`" (byte-identical to a build without the knob; #676).
#[must_use]
pub fn resolve_bundle_main_fields(bundle: Option<&BundleConfig>) -> Vec<String> {
    match bundle {
        Some(b) => b.main_fields.clone().unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Resolve `bundle.external` into the extra `--external` specifiers appended
/// to the page/SSR pass's externals. Empty when `bundle` is absent or
/// `bundle.external` is `None` (#676).
#[must_use]
pub fn resolve_bundle_external(bundle: Option<&BundleConfig>) -> Vec<String> {
    match bundle {
        Some(b) => b.external.clone().unwrap_or_default(),
        None => Vec::new(),
    }
}

// --- helpers ----------------------------------------------------------------

/// Normalise a `base` config value into the prefix string the build
/// concatenates onto the leading `/` of an asset URL like
/// `/assets/styles.css`.
///
/// The contract: `format!("{prefix}{stable_url}")` must produce a
/// well-formed URL where:
///
/// - the joined URL has exactly one `/` between the prefix and the
///   `/assets/...` portion (no doubled slashes, no missing slash).
/// - omitted / empty / `"/"` bases yield an empty prefix, so the
///   pre-`base` build path is byte-identical.
///
/// Accepted authoring shapes (all mapped to a canonical prefix):
///
/// | author wrote          | prefix returned        |
/// |-----------------------|------------------------|
/// | `None` / `""` / `"/"` | `""`                   |
/// | `"/pj/zudo-doc/"`     | `"/pj/zudo-doc"`       |
/// | `"/pj/zudo-doc"`      | `"/pj/zudo-doc"`       |
/// | `"https://cdn.example.com/"` | `"https://cdn.example.com"` |
/// | `"https://cdn.example.com"`  | `"https://cdn.example.com"` |
///
/// We trim trailing slashes (any number of them) so concatenating
/// `prefix + "/assets/..."` always produces a single delimiter. The
/// caller is responsible for ensuring the stable URL it concatenates
/// onto already starts with `/` — the asset-URL constants in
/// `zfb_types::asset_urls` satisfy this by construction.
pub fn asset_url_base_prefix(base: Option<&str>) -> String {
    let Some(raw) = base else {
        return String::new();
    };
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        // Either `""` or `"/"` (or `"//"`, …) — none of these mount the
        // site under a sub-path.
        return String::new();
    }
    trimmed.to_string()
}

// --- defaults --------------------------------------------------------------

fn default_out_dir() -> PathBuf {
    PathBuf::from("dist")
}

fn default_public_dir() -> PathBuf {
    PathBuf::from("public")
}

fn default_true() -> bool {
    true
}

// --- loader ----------------------------------------------------------------
//
// The esbuild + V8 / node TS evaluator was hoisted out of this bin crate into
// the leaf crate `zfb-config-loader` (issue #1037) so `zfb-server` can call it
// too without a dependency cycle (`zfb` depends on `zfb-server`, not the
// reverse). This crate now wraps that loader: it injects the embedded-esbuild
// extraction getter (which needs the `EMBEDDED_VENDOR` snapshot that lives in
// `crate::render_pipeline`), then deserialises the evaluated value into the
// strongly-typed [`Config`] defined above.

/// Knobs that tweak loader behaviour. Public so build/dev/preview can
/// thread an explicit esbuild override through if they ever need to;
/// `Default` is the production path.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Override the esbuild binary path. `None` falls back to
    /// `ZFB_ESBUILD_BIN`, then the embedded esbuild snapshot, then
    /// `zfb_build::DEFAULT_ESBUILD_SLOT`.
    pub esbuild_binary: Option<PathBuf>,
    /// Override the `node` binary. `None` uses `node` from `PATH`.
    pub node_binary: Option<OsString>,
    /// Test-only escape hatch: when `Some`, skip the esbuild + node
    /// subprocesses entirely and treat this string as the JSON form
    /// of `default` from the user's `zfb.config.ts`. Production code
    /// leaves this `None`.
    #[doc(hidden)]
    pub test_default_export_json: Option<String>,
}

impl LoadOptions {
    /// Build the [`zfb_config_loader::LoadOptions`] for a real load, wiring in
    /// the embedded-esbuild extraction getter backed by this crate's
    /// `EMBEDDED_VENDOR` snapshot. A `cargo install`-ed `zfb` binary has no
    /// `crates/zfb/binaries/` workspace dir, so this getter is what lets the
    /// config loader find esbuild without the env var or workspace slot.
    fn to_loader_options(&self) -> zfb_config_loader::LoadOptions {
        zfb_config_loader::LoadOptions {
            esbuild_binary: self.esbuild_binary.clone(),
            node_binary: self.node_binary.clone(),
            embedded_esbuild_getter: Some(Box::new(|| {
                crate::render_pipeline::embedded_binary("esbuild").ok()
            })),
            // The CLI loads and runs plugins — always resolve them. Only the
            // embed API (zfb-server) opts out via `resolve_plugins: false`.
            resolve_plugins: true,
            test_default_export_json: self.test_default_export_json.clone(),
        }
    }
}

/// Load and validate a config from `dir`.
///
/// See the module docs for the resolution order.
pub async fn load_from_dir(dir: &Path) -> Result<Config> {
    load_from_dir_with_options(dir, &LoadOptions::default()).await
}

/// Variant of [`load_from_dir`] with explicit knobs. Most callers want
/// [`load_from_dir`].
pub async fn load_from_dir_with_options(dir: &Path, opts: &LoadOptions) -> Result<Config> {
    let ts_path = dir.join("zfb.config.ts");
    let json_path = dir.join("zfb.config.json");

    // TS wins over JSON — the TypeScript form is the canonical, recommended
    // path for new projects. See module docs for the full resolution order.
    if ts_path.exists() {
        let cfg = load_from_ts_file(&ts_path, dir, opts)
            .await
            .with_context(|| format!("loading {}", ts_path.display()))?;
        validate(&cfg, dir).with_context(|| format!("validating {}", ts_path.display()))?;
        return Ok(cfg);
    }

    if json_path.exists() {
        let text = tokio::fs::read_to_string(&json_path)
            .await
            .with_context(|| format!("reading {}", json_path.display()))?;
        let mut cfg: Config = serde_json::from_str(&text).map_err(|e| {
            anyhow!(
                "{}: invalid config JSON at line {}, column {}: {}",
                json_path.display(),
                e.line(),
                e.column(),
                e
            )
        })?;
        // Issue #211: the JSON config path used to leave every
        // `PluginConfig.resolved_module` at `None`, which made the
        // downstream plugin-host filter silently drop ALL plugins
        // (see `commands::plugins::build_plugin_specs`). Mirror the
        // shape the TS-load path produces (a `file://` URL) for plugin
        // entries that name a path on disk; bare specifiers require Node
        // module resolution (not available on the JSON path) and stay
        // `None` with a user-facing warning so the surprise is visible.
        resolve_json_plugin_modules(&mut cfg, dir)
            .with_context(|| format!("resolving plugin paths for {}", json_path.display()))?;
        validate(&cfg, dir).with_context(|| format!("validating {}", json_path.display()))?;
        return Ok(cfg);
    }

    // No file present → defaults.
    let cfg = Config::default();
    // Defaults are always valid, but we still run the check so future
    // additions can't accidentally break this invariant. Propagate as
    // an error rather than panicking — every config-less project goes
    // through this path and a panic here would tear the dev server
    // down on what is a benign discovery step.
    validate(&cfg, dir).context("Config::default() must validate cleanly")?;
    Ok(cfg)
}

/// Resolve `Config.plugins[].resolved_module` for the JSON-load path.
///
/// Issue #211: `zfb.config.json` is parsed via `serde_json` and skips the
/// TS config evaluator, so without this helper every plugin entry would
/// have `resolved_module = None` and the plugin-host filter
/// (`commands::plugins::build_plugin_specs`) would drop them all silently
/// — both `preBuild` and `postBuild` hooks would never fire.
///
/// Behaviour mirrors the TS evaluator's plugin-resolution logic:
///
/// - Plugin entries naming a relative path (`./` or `../` prefix) or an
///   absolute path are canonicalised against `dir` and converted to a
///   `file://` URL via [`url::Url::from_file_path`]. A missing file is a
///   hard error pointing at the path the user wrote so the failure is
///   self-explanatory.
/// - Bare specifiers (`@scope/pkg`, `pkg-name`) are resolved via
///   [`zfb_config_loader::resolve_node_bare_specifier`] using `oxc_resolver`,
///   which honours conditional exports and parent-directory walk — the same
///   algorithm Node uses for `import.meta.resolve`. A package that cannot be
///   found (not yet installed) is a hard error with a recovery hint rather
///   than a silent skip (issue #211).
///
/// Both the path and bare-specifier resolvers are shared with the TS-load
/// path via `zfb-config-loader` (issue #1037 / #418).
///
/// Fail-fast: the first unresolvable plugin aborts the whole load — surfacing
/// every broken entry at once would just delay the same fix (`pnpm install`
/// or correct the path) by one iteration of the user's edit-build loop.
fn resolve_json_plugin_modules(cfg: &mut Config, dir: &Path) -> Result<()> {
    for entry in cfg.plugins.iter_mut() {
        let name = entry.name.as_str();
        match zfb_config_loader::resolve_plugin_path_to_file_url(name, dir)? {
            Some(url) => {
                entry.resolved_module = Some(url);
            }
            None => {
                // Bare specifier — resolve via oxc_resolver (issue #211 fix).
                // Hard error if the package is not installed so the user gets
                // a clear signal rather than silent plugin-drop.
                let file_url = zfb_config_loader::resolve_node_bare_specifier(name, dir)
                    .with_context(|| {
                        format!(
                            "plugin {:?}: package not found in node_modules \
                             (did you run `pnpm install`?)",
                            name
                        )
                    })?;
                entry.resolved_module = Some(file_url);
            }
        }
    }
    Ok(())
}

/// Load a single `zfb.config.ts` file via the shared
/// [`zfb_config_loader`] crate (issue #1037 hoisted the esbuild + V8 / node
/// evaluator out of this bin crate so `zfb-server` can call it too without a
/// dependency cycle), then deserialise the evaluated `default` export into a
/// strongly-typed [`Config`] and merge the resolved plugin module specifiers
/// back onto `Config.plugins[].resolved_module`.
async fn load_from_ts_file(ts_path: &Path, dir: &Path, opts: &LoadOptions) -> Result<Config> {
    let loaded =
        zfb_config_loader::load_from_ts_file(ts_path, dir, &opts.to_loader_options()).await?;
    parse_loaded_config(loaded, ts_path)
}

/// Deserialise the evaluated config value into [`Config`] and apply the
/// resolved plugin module specifiers (one `file://` URL per `plugins[]`
/// entry, in declaration order).
fn parse_loaded_config(
    loaded: zfb_config_loader::LoadedTsConfig,
    ts_path: &Path,
) -> Result<Config> {
    let zfb_config_loader::LoadedTsConfig {
        config: value,
        resolved_plugins: resolved,
    } = loaded;

    let mut config: Config = serde_json::from_value(value.clone()).map_err(|e| {
        anyhow!(
            "{}: failed to parse the default export as zfb config JSON: {}\n--- received ---\n{}",
            ts_path.display(),
            e,
            value
        )
    })?;

    if !resolved.is_empty() && resolved.len() != config.plugins.len() {
        bail!(
            "{}: plugin resolution count mismatch (config has {} plugins, loader resolved {}); \
             this indicates a bug in the TS config evaluator",
            ts_path.display(),
            config.plugins.len(),
            resolved.len()
        );
    }
    for (entry, resolved_specifier) in config.plugins.iter_mut().zip(resolved) {
        entry.resolved_module = Some(resolved_specifier);
    }
    Ok(config)
}

/// Validate a loaded [`Config`] against the project root `dir`.
///
/// Errors:
/// - duplicate `collections[].name`.
/// - `collections[].path` that is absolute or escapes `dir` via `..`.
fn validate(cfg: &Config, dir: &Path) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for c in &cfg.collections {
        if !seen.insert(c.name.as_str()) {
            bail!("duplicate collection name {:?}", c.name);
        }
        ensure_path_in_root(&c.path, dir).with_context(|| format!("collection {:?}", c.name))?;
    }
    if let Some(ch) = &cfg.code_highlight {
        if let Some(td) = &ch.themes_dir {
            ensure_path_in_root(td, dir).context("codeHighlight.themesDir")?;
        }
        // Dual-theme validation: themeLight and themeDark must be set together.
        match (ch.theme_light.as_ref(), ch.theme_dark.as_ref()) {
            (Some(_), None) | (None, Some(_)) => {
                bail!("codeHighlight.themeLight and themeDark must be set together");
            }
            _ => {}
        }
        // Mutual exclusion: theme and the dual pair cannot both be set.
        if ch.theme.is_some() && (ch.theme_light.is_some() || ch.theme_dark.is_some()) {
            bail!("codeHighlight.theme is mutually exclusive with themeLight/themeDark");
        }
    }
    if let Some(rml) = &cfg.resolve_markdown_links {
        if !rml.docs_dir.as_os_str().is_empty() {
            ensure_path_in_root(&rml.docs_dir, dir).context("resolveMarkdownLinks.docsDir")?;
        }
        for (i, d) in rml.dirs.iter().enumerate() {
            ensure_path_in_root(&d.dir, dir)
                .with_context(|| format!("resolveMarkdownLinks.dirs[{i}].dir"))?;
        }
    }
    if let Some(b) = &cfg.base {
        // An absolute URL is fine ("https://cdn.example.com/..."); a
        // path-style base must start with `/` so the rendered asset
        // URLs (`/pj/foo/assets/...`) match the on-disk dist layout.
        let trimmed = b.trim();
        let looks_absolute_url = trimmed.starts_with("http://") || trimmed.starts_with("https://");
        if !trimmed.is_empty() && !looks_absolute_url && !trimmed.starts_with('/') {
            bail!(
                "base {:?} must start with `/` (e.g. \"/pj/zudo-doc/\") or be an absolute URL",
                b
            );
        }
    }
    for (i, h) in cfg.allowed_hosts.iter().enumerate() {
        // Reject empty entries and a bare "." loudly — a silently dropped
        // entry would surface much later as a confusing 403 on the LAN.
        let trimmed = h.trim();
        if trimmed.is_empty() || trimmed == "." {
            bail!(
                "allowedHosts[{i}]: {:?} is not a valid host entry; use a hostname \
                 (\"example.com\"), a leading-dot subdomain wildcard (\".example.com\"), \
                 or an IP literal",
                h
            );
        }
    }
    for (i, p) in cfg.extra_watch_paths.iter().enumerate() {
        if !p.is_absolute() {
            bail!(
                "extraWatchPaths[{i}]: {:?} must be an absolute path \
                 (e.g. \"/home/user/notes\" or \"/srv/shared-content\"); \
                 relative paths are not accepted because the dev watcher \
                 registers each entry verbatim, outside the project root",
                p
            );
        }
    }
    if let Some(s) = &cfg.site {
        // `site` must be an absolute HTTP/HTTPS URL — it is used to build
        // canonical hrefs, OG URLs, and sitemap entries, none of which make
        // sense with a relative path or non-web scheme.
        let trimmed = s.trim();
        if trimmed.is_empty() {
            bail!("site must not be empty; supply an absolute HTTPS URL (e.g. \"https://example.com\") or omit the field");
        }
        match url::Url::parse(trimmed) {
            Ok(parsed) if parsed.scheme() == "http" || parsed.scheme() == "https" => {}
            Ok(parsed) => bail!(
                "site {:?} uses scheme {:?}; only \"http\" and \"https\" are accepted",
                s,
                parsed.scheme()
            ),
            Err(_) => bail!(
                "site {:?} is not a valid absolute URL; supply an absolute HTTPS URL (e.g. \"https://example.com\")",
                s
            ),
        }
    }
    Ok(())
}

/// Ensure `path` is relative and stays under `dir` (no `..` escapes, no
/// absolute paths). The path is checked **syntactically** — we do not require
/// it to exist, since collections may be created after the config.
///
/// Limitation: symlinks are not resolved here. If the project allows
/// symlinks in content dirs and that's a concern, callers should
/// `canonicalize()` the resolved path at use-site before reading. zfb's v1
/// trust model treats project files as user-owned, so this matches the
/// surrounding crates.
fn ensure_path_in_root(path: &Path, dir: &Path) -> Result<()> {
    if path.is_absolute() {
        bail!(
            "path {:?} must be relative to the project root ({})",
            path,
            dir.display()
        );
    }
    let mut depth: i32 = 0;
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    bail!("path {:?} escapes the project root via `..`", path);
                }
            }
            Component::Normal(_) | Component::CurDir => {
                if matches!(comp, Component::Normal(_)) {
                    depth += 1;
                }
            }
            // Prefix / RootDir would have been caught by `is_absolute` above,
            // but be defensive.
            Component::Prefix(_) | Component::RootDir => {
                bail!(
                    "path {:?} must be relative to the project root ({})",
                    path,
                    dir.display()
                );
            }
        }
    }
    Ok(())
}

// --- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- JsonSchema newtype tests ---------------------------------------------

    #[test]
    fn json_schema_accepts_valid_object_schema() {
        let v = serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "count": { "type": "number" }
            },
            "required": ["title"]
        });
        assert!(JsonSchema::try_from_value(v).is_ok());
    }

    #[test]
    fn json_schema_accepts_known_scalar_types() {
        for ty in &["string", "number", "integer", "boolean", "array", "null"] {
            let v = serde_json::json!({ "type": *ty });
            assert!(
                JsonSchema::try_from_value(v).is_ok(),
                "type {:?} should be accepted",
                ty
            );
        }
    }

    #[test]
    fn json_schema_accepts_type_union_array() {
        let v = serde_json::json!({ "type": ["string", "null"] });
        assert!(JsonSchema::try_from_value(v).is_ok());
    }

    #[test]
    fn json_schema_accepts_empty_object() {
        // An empty schema {} is valid — it accepts anything.
        let v = serde_json::json!({});
        assert!(JsonSchema::try_from_value(v).is_ok());
    }

    #[test]
    fn json_schema_rejects_non_object_root() {
        for bad in &[
            serde_json::json!("string"),
            serde_json::json!(42),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!([1, 2]),
        ] {
            let err = JsonSchema::try_from_value(bad.clone())
                .expect_err("non-object root should be rejected");
            assert!(
                err.contains("must be a JSON object"),
                "unexpected error for {:?}: {err}",
                bad
            );
        }
    }

    #[test]
    fn json_schema_rejects_unknown_type_string() {
        let v = serde_json::json!({ "type": "timestamp" });
        let err = JsonSchema::try_from_value(v).expect_err("unknown type should be rejected");
        assert!(err.contains("\"timestamp\""), "err: {err}");
        assert!(err.contains("not recognised"), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_unknown_type_in_array() {
        let v = serde_json::json!({ "type": ["string", "date"] });
        let err =
            JsonSchema::try_from_value(v).expect_err("unknown type in array should be rejected");
        assert!(err.contains("\"date\""), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_non_string_in_type_array() {
        let v = serde_json::json!({ "type": ["string", 42] });
        let err =
            JsonSchema::try_from_value(v).expect_err("non-string in type array should be rejected");
        assert!(err.contains("must contain strings"), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_non_object_type_field() {
        let v = serde_json::json!({ "type": true });
        let err = JsonSchema::try_from_value(v).expect_err("boolean type field should be rejected");
        assert!(err.contains("must be a string or array"), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_non_object_properties() {
        let v = serde_json::json!({ "type": "object", "properties": ["a", "b"] });
        let err = JsonSchema::try_from_value(v).expect_err("array properties should be rejected");
        assert!(err.contains("\"properties\""), "err: {err}");
        assert!(err.contains("must be a JSON object"), "err: {err}");
    }

    #[test]
    fn json_schema_deref_yields_inner_value() {
        let inner = serde_json::json!({ "type": "string" });
        let js = JsonSchema::try_from_value(inner.clone()).unwrap();
        assert_eq!(*js, inner);
        assert_eq!(js.as_value(), &inner);
    }

    #[test]
    fn json_schema_round_trips_via_serde() {
        // Deserializing a JSON string into JsonSchema should validate and
        // serialize back to the same JSON.
        let raw = r#"{"type":"object","properties":{"title":{"type":"string"}}}"#;
        let js: JsonSchema = serde_json::from_str(raw).unwrap();
        let back = serde_json::to_string(&js).unwrap();
        // The round-trip may reorder keys (BTreeMap) — compare as Values.
        let orig: serde_json::Value = serde_json::from_str(raw).unwrap();
        let roundtripped: serde_json::Value = serde_json::from_str(&back).unwrap();
        assert_eq!(orig, roundtripped);
    }

    #[test]
    fn json_schema_deserialization_rejects_unknown_type() {
        // Serde deserialization path should also reject bad schemas.
        let raw = r#"{"type":"bad-type"}"#;
        let err = serde_json::from_str::<JsonSchema>(raw)
            .expect_err("serde should propagate schema validation error");
        assert!(err.to_string().contains("not recognised"), "err: {err}");
    }

    #[tokio::test]
    async fn config_json_with_valid_schema_loads_ok() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [{
                "name": "blog",
                "path": "content/blog",
                "schema": {
                    "type": "object",
                    "properties": { "title": { "type": "string" } }
                }
            }]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert!(cfg.collections[0].schema.is_some());
    }

    #[tokio::test]
    async fn config_json_with_invalid_schema_type_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [{
                "name": "blog",
                "path": "content/blog",
                "schema": { "type": "nosuchtype" }
            }]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("invalid schema type should be caught at load time");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nosuchtype") || msg.contains("not recognised"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn config_json_with_non_object_schema_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [{
                "name": "blog",
                "path": "content/blog",
                "schema": "not-an-object"
            }]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("non-object schema should be caught at load time");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("must be a JSON object") || msg.contains("invalid type"),
            "msg: {msg}"
        );
    }

    // --- end JsonSchema newtype tests -----------------------------------------

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.out_dir, PathBuf::from("dist"));
        assert_eq!(cfg.public_dir, PathBuf::from("public"));
        assert_eq!(cfg.host, None);
        assert_eq!(cfg.port, None);
        assert_eq!(cfg.framework, Framework::Preact);
        assert!(cfg.collections.is_empty());
        assert!(cfg.tailwind.is_none());
        assert!(cfg.plugins.is_empty());
        // `stripMdExt` is opt-in; absent / default = disabled. Mirrors
        // the Sub 1 outcome byte-for-byte (zfb#127 / #129).
        assert!(!cfg.strip_md_ext);
        // `base` is opt-in; absent => no asset-URL prefix.
        assert_eq!(cfg.base, None);
        // `codeHighlight` is opt-in; absent => default syntect theme.
        assert_eq!(cfg.code_highlight, None);
    }

    // --- base / asset_url_base_prefix tests ----------------------------------

    #[test]
    fn asset_url_base_prefix_none_returns_empty() {
        assert_eq!(asset_url_base_prefix(None), "");
    }

    #[test]
    fn asset_url_base_prefix_empty_string_returns_empty() {
        assert_eq!(asset_url_base_prefix(Some("")), "");
    }

    #[test]
    fn asset_url_base_prefix_root_slash_returns_empty() {
        // `"/"` is the documented shape for "site mounted at the
        // domain root" — the build behaviour must be byte-identical
        // to the no-`base` case.
        assert_eq!(asset_url_base_prefix(Some("/")), "");
    }

    #[test]
    fn asset_url_base_prefix_subpath_strips_trailing_slash() {
        // The PR #1361 acceptance case: `/pj/zudo-doc/` ⇒
        // `/pj/zudo-doc` so concatenation with `/assets/...` produces
        // a single delimiter.
        assert_eq!(asset_url_base_prefix(Some("/pj/zudo-doc/")), "/pj/zudo-doc");
    }

    #[test]
    fn asset_url_base_prefix_subpath_without_trailing_slash_is_idempotent() {
        // Authors who omit the trailing slash get the same prefix as
        // those who include it.
        assert_eq!(asset_url_base_prefix(Some("/pj/zudo-doc")), "/pj/zudo-doc");
    }

    #[test]
    fn asset_url_base_prefix_absolute_url_strips_trailing_slash() {
        // CDN-hosted assets: an absolute URL is mounted onto the
        // asset path. Trailing slash is normalised away so the join
        // is `https://cdn.example.com` + `/assets/...` ⇒ one
        // delimiter, not two.
        assert_eq!(
            asset_url_base_prefix(Some("https://cdn.example.com/")),
            "https://cdn.example.com"
        );
        assert_eq!(
            asset_url_base_prefix(Some("https://cdn.example.com")),
            "https://cdn.example.com"
        );
    }

    #[test]
    fn asset_url_base_prefix_join_produces_well_formed_assets_url() {
        // End-to-end check: the prefix + the authoritative stable URL
        // constant should produce exactly one `/` between them.
        let prefix = asset_url_base_prefix(Some("/pj/zudo-doc/"));
        let joined = format!("{prefix}/assets/styles.css");
        assert_eq!(joined, "/pj/zudo-doc/assets/styles.css");

        let prefix = asset_url_base_prefix(None);
        let joined = format!("{prefix}/assets/styles.css");
        assert_eq!(joined, "/assets/styles.css");

        let prefix = asset_url_base_prefix(Some("/"));
        let joined = format!("{prefix}/assets/styles.css");
        assert_eq!(joined, "/assets/styles.css");

        let prefix = asset_url_base_prefix(Some("https://cdn.example.com/"));
        let joined = format!("{prefix}/assets/styles.css");
        assert_eq!(joined, "https://cdn.example.com/assets/styles.css");
    }

    #[tokio::test]
    async fn loads_base_from_camelcase_json() {
        // The JSON / TS form spells the field `base` (already camel)
        // — confirm round-trip into `Config::base`.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "base": "/pj/zudo-doc/" }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.base.as_deref(), Some("/pj/zudo-doc/"));
    }

    #[tokio::test]
    async fn code_highlight_theme_loads_from_camelcase_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "theme": "InspiredGitHub" } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.theme.as_deref(), Some("InspiredGitHub"));
    }

    #[tokio::test]
    async fn code_highlight_defaults_to_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.code_highlight, None);
    }

    #[tokio::test]
    async fn code_highlight_themes_dir_loads_from_camelcase_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themesDir": "./themes" } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(
            ch.themes_dir.as_deref(),
            Some(std::path::Path::new("./themes"))
        );
    }

    #[tokio::test]
    async fn code_highlight_themes_dir_and_theme_together() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "theme": "Dracula", "themesDir": "themes" } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.theme.as_deref(), Some("Dracula"));
        assert_eq!(
            ch.themes_dir.as_deref(),
            Some(std::path::Path::new("themes"))
        );
    }

    #[tokio::test]
    async fn code_highlight_themes_dir_absolute_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themesDir": "/absolute/path" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("must reject absolute path");
        // anyhow chains context messages; use {:#} to get the full chain.
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.themesDir"),
            "error should mention field; got: {msg}"
        );
    }

    #[tokio::test]
    async fn code_highlight_themes_dir_dotdot_escape_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themesDir": "../../etc" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("must reject .. escape");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.themesDir"),
            "error should mention field; got: {msg}"
        );
    }

    // ── Dual-theme config parse tests (#1067) ─────────────────────────────

    /// `themeLight` and `themeDark` parse from camelCase JSON and populate
    /// the Rust `theme_light` / `theme_dark` fields.
    #[tokio::test]
    async fn code_highlight_theme_light_and_dark_parse_from_camelcase_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themeLight": "base16-ocean.light", "themeDark": "base16-ocean.dark" } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.theme_light.as_deref(), Some("base16-ocean.light"));
        assert_eq!(ch.theme_dark.as_deref(), Some("base16-ocean.dark"));
        assert_eq!(ch.theme, None, "theme must be absent in dual mode");
    }

    /// `themesDir` works alongside `themeLight` + `themeDark`.
    #[tokio::test]
    async fn code_highlight_dual_pair_works_with_themes_dir() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themeLight": "My Light", "themeDark": "My Dark", "themesDir": "themes" } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.theme_light.as_deref(), Some("My Light"));
        assert_eq!(ch.theme_dark.as_deref(), Some("My Dark"));
        assert_eq!(
            ch.themes_dir.as_deref(),
            Some(std::path::Path::new("themes"))
        );
    }

    /// Setting only `themeLight` (without `themeDark`) must be a validation error.
    #[tokio::test]
    async fn code_highlight_only_theme_light_is_error() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themeLight": "base16-ocean.light" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("only themeLight must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("themeLight and themeDark must be set together"),
            "error must mention the mutual-requirement; got: {msg}"
        );
    }

    /// Setting only `themeDark` (without `themeLight`) must be a validation error.
    #[tokio::test]
    async fn code_highlight_only_theme_dark_is_error() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "themeDark": "base16-ocean.dark" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("only themeDark must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("themeLight and themeDark must be set together"),
            "error must mention the mutual-requirement; got: {msg}"
        );
    }

    /// Setting `theme` together with `themeLight` / `themeDark` must be a
    /// validation error (mutually exclusive modes).
    #[tokio::test]
    async fn code_highlight_theme_and_dual_pair_mutually_exclusive() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "theme": "InspiredGitHub", "themeLight": "base16-ocean.light", "themeDark": "base16-ocean.dark" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("theme + dual pair must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("mutually exclusive with themeLight/themeDark"),
            "error must mention mutual exclusion; got: {msg}"
        );
    }

    #[tokio::test]
    async fn base_defaults_to_none_when_absent() {
        // Acceptance criterion: with `base` absent the build must
        // behave byte-for-byte the same as the pre-`base` engine.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.base, None);
    }

    #[tokio::test]
    async fn loads_strip_md_ext_from_camelcase_json() {
        // The JSON / TS form spells the field `stripMdExt`
        // (camelCase). The struct's `#[serde(rename_all = "camelCase")]`
        // attr handles the rename, so a config with `stripMdExt: true`
        // populates `Config::strip_md_ext`.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "stripMdExt": true }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert!(
            cfg.strip_md_ext,
            "stripMdExt: true must deserialise into strip_md_ext = true"
        );
    }

    #[tokio::test]
    async fn strip_md_ext_defaults_to_false_when_absent() {
        // Acceptance criterion: default behaviour must be byte-for-byte
        // identical to Sub 1's outcome — `stripMdExt` absent => false.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert!(!cfg.strip_md_ext);
    }

    #[tokio::test]
    async fn empty_dir_returns_default_config() {
        let tmp = TempDir::new().unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg, Config::default());
    }

    #[tokio::test]
    async fn loads_from_json_fixture() {
        // Test intent: JSON parsing produces a populated Config with all
        // top-level fields (including a plugins entry that round-trips).
        // Plugin resolution itself is covered exhaustively by the
        // json_plugin_* tests; here we use a relative-path plugin (the
        // simplest resolver branch) so the load completes without needing a
        // node_modules fixture. Before #418 this test used a bare-spec
        // ("my-plugin") that the old warn-and-skip path silently dropped;
        // that path now hard-errors, so the fixture switched to a real file.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("plugin.mjs"), "export default {};")
            .await
            .unwrap();
        let json = r#"{
            "outDir": "build",
            "publicDir": "static",
            "host": "0.0.0.0",
            "port": 4000,
            "framework": "react",
            "collections": [
                { "name": "blog", "path": "content/blog" },
                { "name": "docs", "path": "content/docs" }
            ],
            "tailwind": { "enabled": false },
            "plugins": [
                { "name": "./plugin.mjs", "options": { "level": 2 } }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.out_dir, PathBuf::from("build"));
        assert_eq!(cfg.public_dir, PathBuf::from("static"));
        assert_eq!(cfg.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cfg.port, Some(4000));
        assert_eq!(cfg.framework, Framework::React);
        assert_eq!(cfg.collections.len(), 2);
        assert_eq!(cfg.collections[0].name, "blog");
        assert_eq!(cfg.collections[1].path, PathBuf::from("content/docs"));
        assert_eq!(cfg.tailwind, Some(TailwindConfig { enabled: false }));
        assert_eq!(cfg.plugins.len(), 1);
        assert_eq!(cfg.plugins[0].name, "./plugin.mjs");
    }

    #[tokio::test]
    async fn loads_minimal_json_with_defaults() {
        // Empty object should round-trip to all defaults.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg, Config::default());
    }

    #[tokio::test]
    async fn invalid_json_reports_position() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            "{ \"port\": \"not-a-number\" }",
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path()).await.expect_err("should reject");
        let msg = format!("{err:#}");
        assert!(msg.contains("zfb.config.json"), "msg: {msg}");
        assert!(msg.contains("line"), "msg: {msg}");
    }

    #[tokio::test]
    async fn rejects_duplicate_collection_names() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "content/blog" },
                { "name": "blog", "path": "content/blog2" }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject duplicates");
        let msg = format!("{err:#}");
        assert!(msg.contains("duplicate collection name"), "msg: {msg}");
    }

    #[tokio::test]
    async fn rejects_absolute_collection_path() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "/etc/passwd" }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject absolute path");
        let msg = format!("{err:#}");
        assert!(msg.contains("relative"), "msg: {msg}");
    }

    #[tokio::test]
    async fn extra_watch_paths_accepts_absolute_path() {
        // The path does NOT need to exist at load time — existence is
        // checked when the dev watcher canonicalises each entry. The
        // config loader only enforces "absolute or bust".
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "extraWatchPaths": ["/this/path/need/not/exist/at/load/time"]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("absolute path should be accepted");
        assert_eq!(cfg.extra_watch_paths.len(), 1);
        assert_eq!(
            cfg.extra_watch_paths[0],
            PathBuf::from("/this/path/need/not/exist/at/load/time")
        );
    }

    #[tokio::test]
    async fn extra_watch_paths_rejects_relative_path() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "extraWatchPaths": ["./relative/sibling"]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("relative path should be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("extraWatchPaths"),
            "error should name the field: {msg}"
        );
        assert!(
            msg.contains("absolute"),
            "error should mention the absolute-path requirement: {msg}"
        );
    }

    #[tokio::test]
    async fn extra_watch_paths_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{}"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.unwrap();
        assert!(cfg.extra_watch_paths.is_empty());
    }

    #[tokio::test]
    async fn rejects_parent_dir_escape() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "../outside" }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject .. escape");
        let msg = format!("{err:#}");
        assert!(msg.contains("escapes"), "msg: {msg}");
    }

    #[tokio::test]
    async fn allows_internal_dotdot_that_does_not_escape() {
        // `a/../b` resolves to `b` — within the project root, so it should
        // be accepted. (Same shape as `./content/blog`.)
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "a/../b" },
                { "name": "docs", "path": "." }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("should accept");
        assert_eq!(cfg.collections.len(), 2);
    }

    #[tokio::test]
    async fn ts_envelope_populates_resolved_plugin_modules() {
        // The Sub 3 / #108 envelope format: the loader subprocess
        // emits `{ config, plugins: [...resolved-specifiers...] }`.
        // The Rust side merges resolved specifiers onto
        // `Config.plugins[].resolved_module` by index.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                r#"{
                    "config": {
                        "plugins": [
                            { "name": "@example/zfb-plugin-search", "options": { "level": 2 } },
                            { "name": "./plugins/local.mjs" }
                        ]
                    },
                    "plugins": [
                        "file:///abs/node_modules/@example/zfb-plugin-search/index.js",
                        "file:///abs/project/plugins/local.mjs"
                    ]
                }"#
                .to_string(),
            ),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("envelope load ok");
        assert_eq!(cfg.plugins.len(), 2);
        assert_eq!(cfg.plugins[0].name, "@example/zfb-plugin-search");
        assert_eq!(
            cfg.plugins[0].resolved_module.as_deref(),
            Some("file:///abs/node_modules/@example/zfb-plugin-search/index.js"),
        );
        assert_eq!(cfg.plugins[1].name, "./plugins/local.mjs");
        assert_eq!(
            cfg.plugins[1].resolved_module.as_deref(),
            Some("file:///abs/project/plugins/local.mjs"),
        );
    }

    #[tokio::test]
    async fn ts_envelope_count_mismatch_is_rejected() {
        // Defensive guard — if the TS config evaluator ever drifts and
        // emits a plugins array that doesn't 1:1 match config.plugins,
        // we surface that as a clear error rather than silently dropping
        // resolutions.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                r#"{
                    "config": { "plugins": [{ "name": "a" }, { "name": "b" }] },
                    "plugins": ["file:///x/a.mjs"]
                }"#
                .to_string(),
            ),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("count mismatch must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("plugin resolution count mismatch"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn ts_config_loads_via_test_override() {
        // Drives the TS-loading code path WITHOUT running esbuild + node:
        // `LoadOptions::test_default_export_json` short-circuits the
        // subprocess and feeds the canned JSON into the same parse +
        // validate pipeline the production path uses.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.ts"),
            "// pretend TS — content irrelevant for this test\n\
             import { defineConfig } from \"zfb/config\";\n\
             export default defineConfig({ port: 4000 });\n",
        )
        .await
        .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                r#"{"port": 4000, "framework": "react", "collections": [{"name":"blog","path":"content/blog"}]}"#
                    .to_string(),
            ),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("ts loader (mocked) should succeed");
        assert_eq!(cfg.port, Some(4000));
        assert_eq!(cfg.framework, Framework::React);
        assert_eq!(cfg.collections.len(), 1);
        assert_eq!(cfg.collections[0].name, "blog");
    }

    #[tokio::test]
    async fn ts_config_validates_through_same_rules_as_json() {
        // Validation runs after the TS load too — duplicate collection
        // names should still be rejected.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                r#"{"collections":[{"name":"blog","path":"content/blog"},{"name":"blog","path":"content/blog2"}]}"#
                    .to_string(),
            ),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("should reject duplicates from TS too");
        let msg = format!("{err:#}");
        assert!(msg.contains("duplicate collection name"), "msg: {msg}");
    }

    #[tokio::test]
    async fn ts_wins_over_json_when_both_present() {
        // Both files present → TS wins. The JSON file is ignored and the
        // test override is used to inject the canned TS export JSON.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), r#"{"port": 5500}"#)
            .await
            .unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.ts"),
            "import { defineConfig } from \"zfb/config\";\n\
             export default defineConfig({ port: 9999 });\n",
        )
        .await
        .unwrap();
        let opts = LoadOptions {
            // Canned JSON for the TS default export — port 9999 from TS,
            // not 5500 from JSON.
            test_default_export_json: Some(r#"{"port": 9999}"#.into()),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("ts wins, json is ignored");
        assert_eq!(cfg.port, Some(9999));
    }

    #[tokio::test]
    async fn json_used_when_no_ts_present() {
        // Only a JSON file → JSON is loaded (TS is not required).
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), r#"{"port": 5500}"#)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("json load ok");
        assert_eq!(cfg.port, Some(5500));
    }

    // --- JSON plugin module resolution (issue #211) ---------------------------
    //
    // The JSON-load path used to leave every plugin's `resolved_module` at
    // `None`, which the plugin-host filter (commands::plugins::build_plugin_specs)
    // silently dropped — so neither preBuild nor postBuild ever fired for
    // JSON-config projects. These tests pin the new behaviour: relative and
    // absolute path entries get resolved into file:// URLs, missing files
    // become hard errors, and bare specifiers stay None (with a warning) so
    // the user at least sees the surprise.

    #[tokio::test]
    async fn json_plugin_relative_path_resolves_to_file_url() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("plugins");
        tokio::fs::create_dir_all(&plugin_dir).await.unwrap();
        let plugin_path = plugin_dir.join("foo.mjs");
        tokio::fs::write(&plugin_path, "export default {};\n")
            .await
            .unwrap();

        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "plugins": [
                    { "name": "./plugins/foo.mjs", "options": { "k": 1 } }
                ]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path()).await.expect("json load ok");
        assert_eq!(cfg.plugins.len(), 1);

        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("relative-path plugin should populate resolved_module");
        assert!(
            resolved.starts_with("file://"),
            "expected file:// URL, got {resolved:?}"
        );
        // Round-trip the URL back to a path and check it matches the
        // file we created — the canonicalise step normalises symlinks
        // (e.g. /tmp → /private/tmp on macOS) so compare against the
        // canonicalised source path, not the raw `plugin_path`.
        let parsed = url::Url::parse(resolved).expect("valid url");
        let parsed_path = parsed
            .to_file_path()
            .expect("file:// URL should round-trip to a path");
        let canonical_plugin = plugin_path.canonicalize().unwrap();
        assert_eq!(parsed_path, canonical_plugin);
    }

    #[tokio::test]
    async fn json_plugin_missing_file_errors_clearly() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "plugins": [
                    { "name": "./plugins/does-not-exist.mjs" }
                ]
            }"#,
        )
        .await
        .unwrap();

        let err = load_from_dir(tmp.path())
            .await
            .expect_err("missing plugin file should error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("./plugins/does-not-exist.mjs"),
            "error should name the offending plugin entry: {msg}"
        );
        assert!(
            msg.contains("plugin"),
            "error should mention plugin context: {msg}"
        );
    }

    /// Bare specifiers in `zfb.config.json` now resolve via `oxc_resolver`
    /// when `node_modules` is present (issue #211 fix).
    #[tokio::test]
    async fn json_plugin_bare_specifier_resolves_when_installed() {
        let tmp = TempDir::new().unwrap();

        // Set up node_modules/@takazudo/zfb-shell-rename with a minimal
        // package.json + index.js so oxc_resolver can find it.
        let pkg_dir = tmp
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-shell-rename");
        tokio::fs::create_dir_all(&pkg_dir).await.unwrap();
        tokio::fs::write(
            pkg_dir.join("package.json"),
            r#"{ "name": "@takazudo/zfb-shell-rename", "main": "index.js" }"#,
        )
        .await
        .unwrap();
        tokio::fs::write(pkg_dir.join("index.js"), "export default {};\n")
            .await
            .unwrap();

        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "plugins": [
                    { "name": "@takazudo/zfb-shell-rename" }
                ]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("bare specifier with installed package must not error");
        assert_eq!(cfg.plugins.len(), 1);
        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("bare specifier with node_modules present must populate resolved_module");
        assert!(
            resolved.starts_with("file://"),
            "resolved_module should be a file:// URL, got {resolved:?}"
        );
        assert!(
            resolved.contains("zfb-shell-rename"),
            "resolved_module should reference the package, got {resolved:?}"
        );
    }

    /// Bare specifiers for packages that are not installed must produce a
    /// clear error naming the package and giving a recovery hint (issue #211).
    #[tokio::test]
    async fn json_plugin_bare_specifier_errors_clearly_when_not_installed() {
        let tmp = TempDir::new().unwrap();
        // No node_modules — the package cannot be found.
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "plugins": [
                    { "name": "some-uninstalled-pkg" }
                ]
            }"#,
        )
        .await
        .unwrap();

        let err = load_from_dir(tmp.path())
            .await
            .expect_err("uninstalled bare-specifier plugin must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("some-uninstalled-pkg"),
            "error must name the missing package: {msg}"
        );
        assert!(
            msg.contains("pnpm install") || msg.contains("not found"),
            "error must give a recovery hint: {msg}"
        );
    }

    #[tokio::test]
    async fn json_plugin_absolute_path_resolves_to_file_url() {
        // The plugin file lives at an absolute path that's outside the
        // project dir — mirror what a user might do when pointing at a
        // shared monorepo helper. Using TempDir keeps it portable
        // across CI runners that don't have a stable /opt/... layout.
        let plugin_root = TempDir::new().unwrap();
        let plugin_path = plugin_root.path().join("abs-plugin.mjs");
        tokio::fs::write(&plugin_path, "export default {};\n")
            .await
            .unwrap();

        let project = TempDir::new().unwrap();
        let plugin_path_str = plugin_path.canonicalize().unwrap();
        let plugin_path_str = plugin_path_str.to_str().unwrap();
        // The path must be absolute for this branch — assert that
        // before serialising it into JSON.
        assert!(
            plugin_path_str.starts_with('/'),
            "test setup expects POSIX-absolute path, got {plugin_path_str}"
        );
        let json = format!(
            r#"{{ "plugins": [ {{ "name": "{}" }} ] }}"#,
            plugin_path_str
        );
        tokio::fs::write(project.path().join("zfb.config.json"), json)
            .await
            .unwrap();

        let cfg = load_from_dir(project.path())
            .await
            .expect("absolute-path plugin should load");
        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("absolute-path plugin should populate resolved_module");
        let parsed = url::Url::parse(resolved).expect("valid url");
        assert_eq!(parsed.scheme(), "file");
        let parsed_path = parsed
            .to_file_path()
            .expect("file:// URL should round-trip");
        assert_eq!(parsed_path, plugin_path.canonicalize().unwrap());
    }

    // Slim-build-only: the default-features path (embed_v8) never calls
    // node, so the "node not in PATH" error can only fire from the slim
    // build's subprocess fallback (#390 audit-trail criterion 2).
    #[cfg(not(feature = "embed_v8"))]
    #[tokio::test]
    async fn ts_config_missing_node_emits_actionable_error() {
        // Force the node binary to a path that doesn't exist so the
        // spawn fails with NotFound, mirroring what the user sees when
        // node is not on PATH. We also point esbuild at a nonexistent
        // path so we never get past the binary-resolution step on
        // machines where esbuild happens to be installed — the test is
        // checking the shape of the error, not the full pipeline.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        // We need esbuild to succeed first; supply a fake one only if
        // the real default slot exists. Otherwise the esbuild lookup
        // bails first and this assertion would not exercise the node
        // path. Skip in that case — the message we want to test is
        // covered by the resolve_esbuild_binary check anyway.
        if !PathBuf::from(DEFAULT_ESBUILD_SLOT).exists()
            && std::env::var_os("ZFB_ESBUILD_BIN").is_none()
        {
            return;
        }
        let opts = LoadOptions {
            node_binary: Some(OsString::from("zfb-no-such-node-binary-xyz")),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("missing node should error cleanly");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not found in PATH"),
            "msg should call out PATH: {msg}"
        );
        assert!(msg.contains("Node.js"), "msg should mention Node.js: {msg}");
    }

    /// Sub #212 — when neither `LoadOptions::esbuild_binary` nor the
    /// `ZFB_ESBUILD_BIN` env var is set, the resolver falls through to the
    /// embedded extraction tier (the binary staged inside the zfb crate at
    /// build time). The returned path must exist and the function must
    /// hand back a TempDir handle so the caller can keep the extracted
    /// binary alive for the lifetime of the spawned subprocess.
    ///
    /// Exercises the `zfb`-side integration: `LoadOptions::to_loader_options`
    /// wires the `EMBEDDED_VENDOR`-backed getter into the shared resolver in
    /// `zfb-config-loader` (issue #1037).
    #[test]
    fn resolve_esbuild_binary_picks_embedded_path_without_env_or_explicit() {
        // Defensive: if a parent process has set ZFB_ESBUILD_BIN, this
        // test would short-circuit on the env tier instead of testing the
        // embedded tier we care about. Skip cleanly in that case.
        // SAFETY: tests run sequentially within this thread and we do not
        // touch ZFB_ESBUILD_BIN ourselves. Other tests must not depend on
        // a leaked override.
        if std::env::var_os("ZFB_ESBUILD_BIN").is_some() {
            eprintln!(
                "skipping resolve_esbuild_binary_picks_embedded_path_without_env_or_explicit \
                 because ZFB_ESBUILD_BIN is set in the surrounding env"
            );
            return;
        }
        let loader_opts = LoadOptions::default().to_loader_options();
        let (handle, path) = zfb_config_loader::resolve_esbuild_binary_with_handle(&loader_opts)
            .expect("embedded extraction should succeed for esbuild");
        assert!(
            path.exists(),
            "resolved esbuild path should exist: {}",
            path.display()
        );
        // The embedded tier always returns a Some(TempDir). The
        // workspace-relative dev fallback would return None, but we expect
        // the embedded tier to win because the include_dir! snapshot
        // always carries the binary in a release build.
        assert!(
            handle.is_some(),
            "expected the embedded extraction tier to be selected"
        );
    }

    #[tokio::test]
    async fn ts_config_invalid_json_from_subprocess_includes_payload() {
        // When the subprocess emits something that doesn't deserialize,
        // the error includes the received payload to aid debugging.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some("not-json-at-all".into()),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("garbage stdout must not be silently accepted");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("zfb.config.ts"),
            "msg should name the file: {msg}"
        );
        assert!(
            msg.contains("received"),
            "msg should echo the payload: {msg}"
        );
    }

    // --- MarkdownConfig / GFM resolution tests ----------------------------

    // Case 1 of the spec's four-case resolution matrix: `markdown`
    // omitted entirely → conservative default.
    #[test]
    fn gfm_resolve_absent_markdown_yields_conservative_default() {
        let resolved = resolve_gfm_constructs(None);
        assert_eq!(resolved, ResolvedGfmConstructs::CONSERVATIVE);
        // Spell it out so future readers can audit the intent at a
        // glance — strikethrough + table on, everything else off.
        assert!(resolved.strikethrough);
        assert!(resolved.table);
        assert!(!resolved.autolink_literal);
        assert!(!resolved.task_list_item);
        assert!(!resolved.footnote_definition);
    }

    // Same as case 1 but exercising the "user wrote `markdown: {}`"
    // path — every field of MarkdownConfig is None / default, and the
    // overlay should reduce to the conservative base verbatim.
    #[test]
    fn gfm_resolve_empty_markdown_yields_conservative_default() {
        let cfg = MarkdownConfig::default();
        assert_eq!(
            cfg.resolve_constructs(ResolvedGfmConstructs::CONSERVATIVE),
            ResolvedGfmConstructs::CONSERVATIVE
        );
    }

    // Case 2 of the spec's matrix: `gfm: true` → every GFM construct on.
    #[test]
    fn gfm_resolve_shorthand_true_turns_everything_on() {
        let cfg = MarkdownConfig {
            gfm: Some(GfmFlag::All(true)),
            ..MarkdownConfig::default()
        };
        assert_eq!(
            cfg.resolve_constructs(ResolvedGfmConstructs::CONSERVATIVE),
            ResolvedGfmConstructs::ALL_ON
        );
    }

    // Case 3 of the spec's matrix: `gfm: false` → every GFM construct off.
    #[test]
    fn gfm_resolve_shorthand_false_turns_everything_off() {
        let cfg = MarkdownConfig {
            gfm: Some(GfmFlag::All(false)),
            ..MarkdownConfig::default()
        };
        assert_eq!(
            cfg.resolve_constructs(ResolvedGfmConstructs::CONSERVATIVE),
            ResolvedGfmConstructs::ALL_OFF
        );
    }

    // Case 4 of the spec's matrix: partial object —
    // `{ strikethrough: true, autolinkLiteral: false }` — only those
    // fields overlay; absent fields fall back to the conservative
    // default values.
    #[test]
    fn gfm_resolve_partial_object_overlays_only_named_fields() {
        let cfg = MarkdownConfig {
            gfm: Some(GfmFlag::Constructs(GfmConstructs {
                strikethrough: Some(true),
                autolink_literal: Some(false),
                ..GfmConstructs::default()
            })),
            ..MarkdownConfig::default()
        };
        let resolved = cfg.resolve_constructs(ResolvedGfmConstructs::CONSERVATIVE);
        // Named explicitly:
        assert!(resolved.strikethrough);
        assert!(!resolved.autolink_literal);
        // Unnamed → conservative defaults:
        assert!(resolved.table);
        assert!(!resolved.task_list_item);
        assert!(!resolved.footnote_definition);
    }

    // A partial object that flips a single conservative-default field
    // OFF (e.g. `{ table: false }`) — verifies the overlay can move a
    // field in either direction.
    #[test]
    fn gfm_resolve_partial_object_can_turn_off_conservative_defaults() {
        let cfg = MarkdownConfig {
            gfm: Some(GfmFlag::Constructs(GfmConstructs {
                table: Some(false),
                ..GfmConstructs::default()
            })),
            ..MarkdownConfig::default()
        };
        let resolved = cfg.resolve_constructs(ResolvedGfmConstructs::CONSERVATIVE);
        assert!(resolved.strikethrough); // conservative-default stayed
        assert!(!resolved.table); // explicit override
    }

    // Serde round-trip — the TS `gfm: true` shorthand and the
    // TS `gfm: { strikethrough: true }` partial object must both
    // deserialise into the right variant.
    #[test]
    fn markdown_config_deserialises_from_shorthand_and_partial_object() {
        // Shorthand boolean form.
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "gfm": true
        }))
        .expect("shorthand true deserialises");
        assert_eq!(cfg.gfm, Some(GfmFlag::All(true)));

        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "gfm": false
        }))
        .expect("shorthand false deserialises");
        assert_eq!(cfg.gfm, Some(GfmFlag::All(false)));

        // Partial object form — TS camelCase round-trips into Rust
        // snake_case via the parent struct's `rename_all = "camelCase"`.
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "gfm": {
                "strikethrough": true,
                "autolinkLiteral": false
            }
        }))
        .expect("partial object deserialises");
        assert_eq!(
            cfg.gfm,
            Some(GfmFlag::Constructs(GfmConstructs {
                strikethrough: Some(true),
                autolink_literal: Some(false),
                ..GfmConstructs::default()
            }))
        );
    }

    // The top-level `Config` accepts the `markdown` field via the
    // camelCase rename — confirming integration with the rest of the
    // config struct.
    #[test]
    fn config_top_level_accepts_markdown_field() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "markdown": { "gfm": true }
        }))
        .expect("top-level markdown deserialises");
        assert_eq!(
            cfg.markdown,
            Some(MarkdownConfig {
                gfm: Some(GfmFlag::All(true)),
                toc: None,
                external_links: None,
                cjk_friendly: None,
                hard_breaks: None,
                features: None,
            })
        );
    }

    // --- allowedHosts field tests (#931) --------------------------------------

    #[tokio::test]
    async fn allowed_hosts_loads_camel_case_entries() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "allowedHosts": ["example.com", ".sub.example.org", "[::1]"] }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(
            cfg.allowed_hosts,
            vec![
                "example.com".to_string(),
                ".sub.example.org".to_string(),
                "[::1]".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn allowed_hosts_defaults_to_empty_when_absent() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert!(cfg.allowed_hosts.is_empty());
    }

    #[tokio::test]
    async fn allowed_hosts_rejects_empty_and_bare_dot_entries() {
        for bad in [
            r#"{ "allowedHosts": [""] }"#,
            r#"{ "allowedHosts": ["."] }"#,
        ] {
            let tmp = TempDir::new().unwrap();
            tokio::fs::write(tmp.path().join("zfb.config.json"), bad)
                .await
                .unwrap();
            let err = load_from_dir(tmp.path())
                .await
                .expect_err("invalid allowedHosts entry should be rejected");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("allowedHosts"),
                "expected error mentioning allowedHosts; got: {msg}"
            );
        }
    }

    // --- site field tests (#254) --------------------------------------------

    #[tokio::test]
    async fn site_loads_valid_https_url() {
        // Present-and-valid: a well-formed HTTPS URL loads without error
        // and round-trips into `Config::site`.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "site": "https://example.com" }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.site.as_deref(), Some("https://example.com"));
    }

    #[tokio::test]
    async fn site_loads_valid_http_url() {
        // HTTP is accepted in addition to HTTPS (e.g. local/intranet sites).
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "site": "http://localhost:3000" }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.site.as_deref(), Some("http://localhost:3000"));
    }

    #[tokio::test]
    async fn site_defaults_to_none_when_absent() {
        // Absent `site` must produce `None` — the build is byte-for-byte
        // identical to the pre-`site` build (sub #254 parity criterion).
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.site, None);
    }

    #[tokio::test]
    async fn site_rejects_relative_url() {
        // A relative path is not an absolute URL and must be rejected.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "site": "/pj/my-site/" }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("relative path should be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("site")
                && (msg.contains("not a valid absolute URL") || msg.contains("scheme")),
            "expected error mentioning 'site' and URL validity; got: {msg}"
        );
    }

    #[tokio::test]
    async fn site_rejects_empty_string() {
        // Empty string has no semantic value for a canonical URL.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), r#"{ "site": "" }"#)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("empty string should be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("site") && msg.contains("not be empty"),
            "expected error mentioning 'site' and emptiness; got: {msg}"
        );
    }

    #[tokio::test]
    async fn site_rejects_non_http_scheme() {
        // Non-HTTP(S) schemes like `ftp://` are semantically wrong for
        // a web canonical URL.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "site": "ftp://example.com" }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("ftp:// scheme should be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("site") && msg.contains("ftp"),
            "expected error mentioning 'site' and 'ftp'; got: {msg}"
        );
    }

    // --- PrefetchConfig field tests (#277) -----------------------------------

    #[tokio::test]
    async fn prefetch_disabled_true_parses_correctly() {
        // `{ "prefetch": { "disabled": true } }` must parse to
        // `Some(PrefetchConfig { disabled: Some(true) })`.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "prefetch": { "disabled": true } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(
            cfg.prefetch,
            Some(PrefetchConfig {
                disabled: Some(true)
            }),
        );
    }

    #[tokio::test]
    async fn prefetch_defaults_to_none_when_absent() {
        // Absent `prefetch` block must produce `None` — the build is
        // byte-for-byte identical to the pre-`prefetch` build (parity
        // criterion).
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.prefetch, None);
    }

    // --- resolve_cjk_friendly tests ---

    // Absent `markdown` block → CJK-friendly is on (default-true).
    #[test]
    fn cjk_friendly_absent_markdown_yields_true() {
        assert!(resolve_cjk_friendly(None));
    }

    // Empty `MarkdownConfig` → no cjk_friendly field → default true.
    #[test]
    fn cjk_friendly_empty_markdown_yields_true() {
        let cfg = MarkdownConfig::default();
        assert!(resolve_cjk_friendly(Some(&cfg)));
    }

    // `cjkFriendly: true` — explicit opt-in.
    #[test]
    fn cjk_friendly_explicit_true() {
        let cfg = MarkdownConfig {
            cjk_friendly: Some(true),
            ..MarkdownConfig::default()
        };
        assert!(resolve_cjk_friendly(Some(&cfg)));
    }

    // `cjkFriendly: false` — opt-out.
    #[test]
    fn cjk_friendly_explicit_false() {
        let cfg = MarkdownConfig {
            cjk_friendly: Some(false),
            ..MarkdownConfig::default()
        };
        assert!(!resolve_cjk_friendly(Some(&cfg)));
    }

    // Serde: `cjkFriendly` camelCase round-trips from JSON.
    #[test]
    fn cjk_friendly_deserialises_from_camel_case() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "cjkFriendly": false
        }))
        .expect("cjkFriendly:false deserialises");
        assert_eq!(cfg.cjk_friendly, Some(false));

        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "cjkFriendly": true
        }))
        .expect("cjkFriendly:true deserialises");
        assert_eq!(cfg.cjk_friendly, Some(true));

        // Absent field → None (default-on at resolve time).
        let cfg: MarkdownConfig =
            serde_json::from_value(serde_json::json!({})).expect("empty object deserialises");
        assert_eq!(cfg.cjk_friendly, None);
    }

    // --- resolve_hard_breaks tests ---

    // Absent `markdown` block → hard breaks off (default false).
    #[test]
    fn hard_breaks_absent_markdown_yields_false() {
        assert!(!resolve_hard_breaks(None));
    }

    // Empty `MarkdownConfig` → no hard_breaks field → default false.
    #[test]
    fn hard_breaks_empty_markdown_yields_false() {
        let cfg = MarkdownConfig::default();
        assert!(!resolve_hard_breaks(Some(&cfg)));
    }

    // `hardBreaks: true` — explicit opt-in.
    #[test]
    fn hard_breaks_explicit_true() {
        let cfg = MarkdownConfig {
            hard_breaks: Some(true),
            ..MarkdownConfig::default()
        };
        assert!(resolve_hard_breaks(Some(&cfg)));
    }

    // `hardBreaks: false` — explicit opt-out (same as absent).
    #[test]
    fn hard_breaks_explicit_false() {
        let cfg = MarkdownConfig {
            hard_breaks: Some(false),
            ..MarkdownConfig::default()
        };
        assert!(!resolve_hard_breaks(Some(&cfg)));
    }

    // Serde: `hardBreaks` camelCase round-trips from JSON.
    #[test]
    fn hard_breaks_deserialises_from_camel_case() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "hardBreaks": true
        }))
        .expect("hardBreaks:true deserialises");
        assert_eq!(cfg.hard_breaks, Some(true));

        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "hardBreaks": false
        }))
        .expect("hardBreaks:false deserialises");
        assert_eq!(cfg.hard_breaks, Some(false));

        // Absent field → None (default-off at resolve time).
        let cfg: MarkdownConfig =
            serde_json::from_value(serde_json::json!({})).expect("empty object deserialises");
        assert_eq!(cfg.hard_breaks, None);
    }

    // --- bundle.exclude tests (#664 / #672) --------------------------------

    #[test]
    fn bundle_exclude_deserialises_from_camel_case() {
        // The wire shape `zfb.config.ts` hands us: `{ "bundle": { "exclude": [...] } }`.
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "bundle": { "exclude": ["components/*.stories.tsx"] }
        }))
        .expect("bundle.exclude deserialises");
        assert_eq!(
            cfg.bundle.as_ref().and_then(|b| b.exclude.clone()),
            Some(vec!["components/*.stories.tsx".to_string()])
        );
        // Resolver returns the list verbatim.
        assert_eq!(
            resolve_bundle_exclude(cfg.bundle.as_ref()),
            vec!["components/*.stories.tsx".to_string()]
        );
    }

    #[test]
    fn bundle_absent_resolves_to_empty_exclude() {
        // Absent `bundle` key → None → resolver yields empty (skip nothing,
        // byte-identical to a build without the knob).
        let cfg: Config =
            serde_json::from_value(serde_json::json!({})).expect("empty config deserialises");
        assert!(cfg.bundle.is_none());
        assert!(resolve_bundle_exclude(cfg.bundle.as_ref()).is_empty());

        // `bundle: {}` with no `exclude` also resolves empty.
        let cfg: Config = serde_json::from_value(serde_json::json!({ "bundle": {} }))
            .expect("bundle:{} deserialises");
        assert!(resolve_bundle_exclude(cfg.bundle.as_ref()).is_empty());
    }

    // --- bundle.mainFields / bundle.external tests (#676) ------------------

    #[test]
    fn bundle_main_fields_and_external_deserialise_from_camel_case() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "bundle": { "mainFields": ["main", "module"], "external": ["path-to-regexp"] }
        }))
        .expect("bundle.mainFields / bundle.external deserialise");
        assert_eq!(
            resolve_bundle_main_fields(cfg.bundle.as_ref()),
            vec!["main".to_string(), "module".to_string()]
        );
        assert_eq!(
            resolve_bundle_external(cfg.bundle.as_ref()),
            vec!["path-to-regexp".to_string()]
        );
    }

    #[test]
    fn bundle_absent_resolves_to_empty_main_fields_and_external() {
        let cfg: Config =
            serde_json::from_value(serde_json::json!({})).expect("empty config deserialises");
        assert!(resolve_bundle_main_fields(cfg.bundle.as_ref()).is_empty());
        assert!(resolve_bundle_external(cfg.bundle.as_ref()).is_empty());

        let cfg: Config = serde_json::from_value(serde_json::json!({ "bundle": {} }))
            .expect("bundle:{} deserialises");
        assert!(resolve_bundle_main_fields(cfg.bundle.as_ref()).is_empty());
        assert!(resolve_bundle_external(cfg.bundle.as_ref()).is_empty());
    }

    // --- OutputMode tests (sub-task 4.1b / issue #373) ---------------------

    #[test]
    fn output_mode_default_is_auto() {
        // Default-derive on the enum picks the `#[default]` variant; the
        // top-level `Config::default()` then propagates it.
        assert_eq!(OutputMode::default(), OutputMode::Auto);
        let cfg = Config::default();
        assert_eq!(cfg.output, OutputMode::Auto);
    }

    #[test]
    fn output_mode_serde_roundtrip_lowercase() {
        // The serde `rename_all = "lowercase"` shape must round-trip the
        // three variants verbatim — this is the wire contract zfb.config.ts
        // / zfb.config.json hand to us.
        for (variant, wire) in [
            (OutputMode::Static, "static"),
            (OutputMode::Hybrid, "hybrid"),
            (OutputMode::Auto, "auto"),
        ] {
            let json = serde_json::to_string(&variant).expect("serialise OutputMode");
            assert_eq!(json, format!("\"{wire}\""));
            let parsed: OutputMode = serde_json::from_str(&json).expect("deserialise OutputMode");
            assert_eq!(parsed, variant);
        }
    }

    #[tokio::test]
    async fn output_absent_in_json_defaults_to_auto() {
        // Existing configs that pre-date 4.1b have no `output` field — they
        // must still load and resolve to Auto so the build picks the
        // detection-driven path.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.output, OutputMode::Auto);
    }

    #[tokio::test]
    async fn output_static_loads_from_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "output": "static" }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.output, OutputMode::Static);
    }

    #[tokio::test]
    async fn output_hybrid_loads_from_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "output": "hybrid" }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.output, OutputMode::Hybrid);
    }

    #[tokio::test]
    async fn output_auto_loads_from_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "output": "auto" }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.output, OutputMode::Auto);
    }

    #[tokio::test]
    async fn output_unknown_variant_is_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), r#"{ "output": "ssr" }"#)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("unknown output variant should be rejected at load time");
        let msg = format!("{err:#}");
        assert!(msg.contains("ssr") || msg.contains("variant"), "msg: {msg}");
    }

    // --- Sub #417 V8 evaluator wiring tests ----------------------------------

    /// `spawn_blocking` in the V8 eval path keeps the tokio event loop free.
    ///
    /// ## What is tested
    ///
    /// `config.rs` wraps `ThreadedConfigEvaluator::eval_bundle` inside
    /// `tokio::task::spawn_blocking` so the synchronous `rx.recv()` call
    /// (blocking the calling thread until the JS eval finishes) does not pin
    /// the single-threaded tokio event loop. This test goes through the full
    /// production `load_from_dir` path and detects a dropped `spawn_blocking`
    /// by measuring the maximum event-loop starvation gap.
    ///
    /// ## Detection mechanism
    ///
    /// A heartbeat probe runs concurrently with `load_from_dir` via
    /// `tokio::join!`. The probe loops, sleeping 2 ms on each iteration and
    /// recording the wall-clock gap since the previous wakeup. The worst gap
    /// observed over the entire load captures whether the event loop was ever
    /// frozen for a long time.
    ///
    /// - **With `spawn_blocking`:** eval_bundle's `rx.recv()` runs on the
    ///   blocking thread pool; the event loop stays free; the probe wakes on
    ///   schedule; `max_gap` ≈ probe interval + jitter (small, a few ms).
    /// - **Without `spawn_blocking`:** `rx.recv()` freezes the current-thread
    ///   event loop for the full JS eval duration; the probe cannot be polled
    ///   during that window; `max_gap` ≈ JS eval duration (large, ~150 ms).
    ///
    /// ## Why `current_thread`
    ///
    /// A `multi_thread` runtime has spare workers that can advance the probe
    /// even when one worker is blocked. `current_thread` forces a single event
    /// loop, making the starvation effect visible.
    ///
    /// ## Why the config has a CPU-heavy warm-up expression
    ///
    /// deno_core ships a **V8 snapshot** — V8 boot is pre-baked into the
    /// binary. Natural eval on warm V8 takes only ~20 ms, too short and too
    /// timing-sensitive to discriminate WITH vs WITHOUT spawn_blocking. A
    /// deliberate CPU-busy loop embedded in the config's default export forces
    /// the eval to take ~150–200 ms deterministically on every attempt (V8
    /// cannot DCE it because the result is part of the exported object). This
    /// wide, stable signal makes the threshold reliable and removes the flake
    /// root cause that affected the original test.
    ///
    /// ## Retry rationale
    ///
    /// Under heavy OS scheduler load the probe gap can be elevated even with
    /// `spawn_blocking` in place (transient false failure). A bounded retry
    /// loop (up to `MAX_ATTEMPTS`) passes as soon as ANY attempt's `max_gap`
    /// is below the threshold, and only panics if ALL attempts exceed it. This
    /// tolerates transient spikes while still detecting a removed
    /// `spawn_blocking`: every attempt would have a large gap (the full JS
    /// eval duration ~150–200 ms), so no retry would ever pass the threshold.
    ///
    /// ## Threshold
    ///
    /// The threshold (75 ms) sits cleanly between the WITH case (typically
    /// < 15 ms) and the WITHOUT case (~150 ms). The original 10× sleep
    /// tolerance was too wide — this test needs a gap-based measurement to
    /// discriminate the two cases.
    #[cfg(feature = "embed_v8")]
    #[tokio::test(flavor = "current_thread")]
    async fn v8_eval_spawn_blocking_does_not_starve_event_loop() {
        use std::cell::Cell;
        use std::rc::Rc;

        const MAX_ATTEMPTS: u32 = 5;
        // Threshold between WITH (~few ms) and WITHOUT (~150+ ms).
        // 75 ms provides a wide, stable margin between the two cases.
        let gap_threshold = std::time::Duration::from_millis(75);

        // CPU-heavy config: the IIFE runs a busy loop whose result is part of
        // the exported object, preventing V8 dead-code elimination.
        // This forces the eval to take ~150-200 ms regardless of V8 warm/cold
        // state — necessary because deno_core's snapshot makes natural eval
        // only ~20 ms, which is too short to discriminate spawn_blocking
        // presence from absence. The `_warmup` field is ignored by assertions.
        // iteration count: 2e8 ≈ 150-200 ms on modern hardware (calibrated).
        const HEAVY_CONFIG: &str = "\
            export default { port: 7777, \
                _warmup: (() => { let s = 0; for (let i = 0; i < 2e8; i++) s += i; return s; })() \
            };";

        let mut last_max_gap = std::time::Duration::ZERO;

        for attempt in 1..=MAX_ATTEMPTS {
            let tmp = TempDir::new().unwrap();
            tokio::fs::write(tmp.path().join("zfb.config.ts"), HEAVY_CONFIG)
                .await
                .unwrap();

            let done = Rc::new(Cell::new(false));
            let probe_done = done.clone();

            // Heartbeat probe: runs while load_from_dir is active.
            // Records the maximum gap between successive wakeups, which is a
            // proxy for how long the event loop was unavailable.
            //
            // The loop checks `probe_done` AFTER measuring, not before, so
            // it always records the gap that ends the final blocking window.
            let probe = async move {
                let probe_interval = std::time::Duration::from_millis(2);
                let mut max_gap = std::time::Duration::ZERO;
                let mut last = std::time::Instant::now();
                loop {
                    tokio::time::sleep(probe_interval).await;
                    let now = std::time::Instant::now();
                    let gap = now.duration_since(last);
                    if gap > max_gap {
                        max_gap = gap;
                    }
                    last = now;
                    if probe_done.get() {
                        break;
                    }
                }
                max_gap
            };

            let load = async {
                let result = load_from_dir(tmp.path()).await;
                done.set(true);
                result
            };

            let (load_result, max_gap) = tokio::join!(load, probe);

            // The load must succeed on every attempt.
            let cfg = load_result.expect("V8 eval should load a simple config");
            assert_eq!(cfg.port, Some(7777));

            last_max_gap = max_gap;

            if max_gap < gap_threshold {
                // Event loop stayed free — spawn_blocking is in place.
                return;
            }

            eprintln!(
                "attempt {attempt}/{MAX_ATTEMPTS}: max_gap={max_gap:?} \
                 (threshold {gap_threshold:?}) — retrying"
            );
        }

        // Every attempt had a large event-loop gap. With spawn_blocking
        // present the gap is tiny (probe jitter only). This large gap means
        // the event loop was frozen — almost certainly because spawn_blocking
        // was removed and eval_bundle's rx.recv() ran inline.
        panic!(
            "max event-loop gap was {last_max_gap:?} on all {MAX_ATTEMPTS} attempts \
             (threshold {gap_threshold:?}) — the tokio current-thread event loop \
             was frozen; spawn_blocking may have been removed from the V8 eval \
             path in config.rs"
        );
    }

    /// esbuild `--platform=neutral` rejects `node:fs` at bundle time.
    ///
    /// Asserts that a `zfb.config.ts` containing `import fs from "node:fs"`
    /// produces an esbuild bundle-time error (not a V8 eval error) and that
    /// the error message names the offending import. This is the data-config
    /// contract: `zfb.config.ts` must not import Node built-ins.
    #[cfg(feature = "embed_v8")]
    #[tokio::test]
    async fn v8_eval_node_fs_import_fails_at_bundle_time_not_v8() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.ts"),
            // data-config contract: node:* imports are forbidden.
            "import fs from \"node:fs\";\nexport default { files: fs.readdirSync(\".\") };\n",
        )
        .await
        .unwrap();

        let err = load_from_dir(tmp.path())
            .await
            .expect_err("node:fs import must fail at bundle time");
        let msg = format!("{err:#}");

        // Must be an esbuild error, not a V8 eval error.
        assert!(
            msg.contains("esbuild failed"),
            "error should come from esbuild (bundle time), got: {msg}"
        );
        // Must name the offending import so users know what to fix.
        assert!(
            msg.contains("node:fs"),
            "error should name the offending node:fs import, got: {msg}"
        );
        // Must NOT say "embedded V8 evaluator failed" — the error must
        // occur before V8 is even invoked.
        assert!(
            !msg.contains("embedded V8 evaluator failed"),
            "error must not come from V8 eval (must be bundle-time), got: {msg}"
        );
    }

    /// Slim-build only: verify that the "node not in PATH" actionable error
    /// fires when both node is missing AND the slim-build fallback is used.
    ///
    /// Gated under `cfg(not(feature = "embed_v8"))` because the default-features
    /// build compiles V8 in and never calls node to evaluate config — this
    /// state (V8 absent AND node absent) cannot occur on the default path.
    /// (#390 audit-trail: second acceptance criterion.)
    #[cfg(not(feature = "embed_v8"))]
    #[tokio::test]
    async fn slim_build_missing_node_emits_actionable_error() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        if !PathBuf::from(DEFAULT_ESBUILD_SLOT).exists()
            && std::env::var_os("ZFB_ESBUILD_BIN").is_none()
        {
            return;
        }
        let opts = LoadOptions {
            node_binary: Some(OsString::from("zfb-no-such-node-binary-slim-xyz")),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("missing node should error cleanly on slim build");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not found in PATH"),
            "msg should call out PATH: {msg}"
        );
        assert!(msg.contains("Node.js"), "msg should mention Node.js: {msg}");
    }

    // --- MarkdownFeaturesConfig tests (#566) ---------------------------------

    // Absent `features` field → `None` (all features disabled; the four
    // former-Core framework features are off by default — #583 / #586).
    #[test]
    fn features_absent_yields_none() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({}))
            .expect("empty MarkdownConfig deserialises");
        assert_eq!(cfg.features, None);
    }

    // `features: {}` — explicit empty object → `Some(MarkdownFeaturesConfig::default())`.
    #[test]
    fn features_empty_object_yields_default() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": {}
        }))
        .expect("empty features object deserialises");
        assert_eq!(cfg.features, Some(MarkdownFeaturesConfig::default()));
    }

    // Boolean `true` shorthand: `githubAlerts: true` → `FeatureToggle::Bool(true)`.
    #[test]
    fn features_github_alerts_true_shorthand() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "githubAlerts": true }
        }))
        .expect("githubAlerts: true deserialises");
        let features = cfg.features.expect("features present");
        assert_eq!(features.github_alerts, Some(FeatureToggle::Bool(true)));
    }

    // Boolean `false` shorthand: `githubAlerts: false` → `FeatureToggle::Bool(false)`.
    #[test]
    fn features_github_alerts_false_shorthand() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "githubAlerts": false }
        }))
        .expect("githubAlerts: false deserialises");
        let features = cfg.features.expect("features present");
        assert_eq!(features.github_alerts, Some(FeatureToggle::Bool(false)));
    }

    // Object form: `githubAlerts: {}` → `FeatureToggle::Options(FeatureOptions {})`.
    #[test]
    fn features_github_alerts_object_form() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "githubAlerts": {} }
        }))
        .expect("githubAlerts: {} deserialises");
        let features = cfg.features.expect("features present");
        assert_eq!(
            features.github_alerts,
            Some(FeatureToggle::Options(FeatureOptions {}))
        );
    }

    // readingTime: { wpm: 250 } → ReadingTimeFeature::Options(ReadingTimeOptions { wpm: Some(250) }).
    #[test]
    fn features_reading_time_object_form() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "readingTime": { "wpm": 250 } }
        }))
        .expect("readingTime: { wpm: 250 } deserialises");
        let features = cfg.features.expect("features present");
        assert_eq!(
            features.reading_time,
            Some(ReadingTimeFeature::Options(ReadingTimeOptions {
                wpm: Some(250)
            }))
        );
    }

    // readingTime: true → ReadingTimeFeature::Bool(true).
    #[test]
    fn features_reading_time_bool_true() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "readingTime": true }
        }))
        .expect("readingTime: true deserialises");
        let features = cfg.features.expect("features present");
        assert_eq!(features.reading_time, Some(ReadingTimeFeature::Bool(true)));
    }

    // readingTime: { bogus: true } must be rejected. `deny_unknown_fields` on
    // ReadingTimeOptions makes the `Options` variant reject the unknown `bogus`
    // key, and the value is not a bool, so the untagged enum matches no variant.
    // (An untagged enum surfaces a generic "did not match any variant" error
    // rather than naming the offending field — the rejection is what matters.)
    #[test]
    fn features_reading_time_unknown_field_is_rejected() {
        let err = serde_json::from_value::<MarkdownConfig>(serde_json::json!({
            "features": { "readingTime": { "bogus": true } }
        }))
        .expect_err("readingTime: { bogus: true } must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("did not match any variant")
                || msg.contains("bogus")
                || msg.contains("unknown field"),
            "unknown field must cause rejection; got: {msg}"
        );
    }

    // `githubAutolinks` uses a dedicated struct (requires `repo` field).
    #[test]
    fn features_github_autolinks_with_repo() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "githubAutolinks": { "repo": "owner/repo" } }
        }))
        .expect("githubAutolinks deserialises");
        let features = cfg.features.expect("features present");
        let autolinks = features.github_autolinks.expect("githubAutolinks present");
        assert_eq!(autolinks.repo.as_deref(), Some("owner/repo"));
    }

    // Multiple features enabled simultaneously in one `features` block.
    #[test]
    fn features_multiple_features_in_one_block() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": {
                "githubAlerts": true,
                "mermaid": true,
                "ruby": false
            }
        }))
        .expect("multiple features deserialise");
        let features = cfg.features.expect("features present");
        assert_eq!(features.github_alerts, Some(FeatureToggle::Bool(true)));
        assert_eq!(features.mermaid, Some(FeatureToggle::Bool(true)));
        assert_eq!(features.ruby, Some(FeatureToggle::Bool(false)));
        // Others absent → None.
        assert_eq!(features.code_tabs, None);
    }

    // Unknown feature key must be rejected with a clear error.
    // This is the spec acceptance criterion: `features: { bogus: true }`
    // must produce a deserialization error naming the unknown field.
    #[test]
    fn features_unknown_key_is_rejected() {
        let err = serde_json::from_value::<MarkdownConfig>(serde_json::json!({
            "features": { "bogus": true }
        }))
        .expect_err("unknown feature key must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("bogus") || msg.contains("unknown field"),
            "error must name the unknown field; got: {msg}"
        );
    }

    // Top-level Config integration: `markdown.features.githubAlerts: true`
    // must parse end-to-end via `serde_json::from_value::<Config>`.
    #[test]
    fn config_markdown_features_github_alerts_roundtrip() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "markdown": { "features": { "githubAlerts": true } }
        }))
        .expect("Config with markdown.features.githubAlerts deserialises");
        let features = cfg
            .markdown
            .as_ref()
            .expect("markdown present")
            .features
            .as_ref()
            .expect("features present");
        assert_eq!(features.github_alerts, Some(FeatureToggle::Bool(true)));
    }

    // `features` absent from the top-level `markdown` block → `None`.
    // This ensures existing configs that pre-date the `features` field
    // still load without errors.
    #[test]
    fn config_markdown_features_absent_yields_none() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "markdown": { "gfm": true }
        }))
        .expect("Config without features field deserialises");
        let markdown = cfg.markdown.as_ref().expect("markdown present");
        assert_eq!(markdown.features, None);
    }

    // Unknown keys inside a per-feature OBJECT must reject too — without
    // `deny_unknown_fields` on the option structs, the untagged
    // `FeatureToggle` enum would silently accept `{ bogus: true }` as
    // `Options(FeatureOptions {})` and the feature would turn on
    // unintentionally. Codex review flag in #564 Wave 1.
    //
    // Note: with `#[serde(untagged)]` on `FeatureToggle`, serde swallows
    // the inner "unknown field" message and reports only the outer
    // "did not match any variant" error — so we assert on the variant
    // text rather than the field name itself.
    #[test]
    fn features_per_feature_object_rejects_unknown_keys() {
        let err = serde_json::from_value::<MarkdownConfig>(serde_json::json!({
            "features": { "githubAlerts": { "bogus": true } }
        }))
        .expect_err("unknown keys inside the per-feature object must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("FeatureToggle") || msg.contains("variant"),
            "error must indicate the FeatureToggle variant rejection; got: {msg}"
        );
    }

    // Same shape via the typed-options struct (`GithubAutolinksConfig`)
    // — unknown keys alongside the legitimate `repo` field must reject.
    // `GithubAutolinksConfig` is NOT inside an untagged enum, so the
    // inner "unknown field" message survives and we can assert on it.
    #[test]
    fn features_typed_option_struct_rejects_unknown_keys() {
        let err = serde_json::from_value::<MarkdownConfig>(serde_json::json!({
            "features": { "githubAutolinks": { "repo": "owner/repo", "bogus": true } }
        }))
        .expect_err("unknown keys inside a typed feature option struct must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("bogus") || msg.contains("unknown field"),
            "error must name the unknown field; got: {msg}"
        );
    }

    // --- directives round-trip tests -------------------------------------------

    // `directives: { spoiler: "Spoiler" }` — short-form entry.
    #[test]
    fn directives_short_form_round_trip() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": { "directives": { "spoiler": "Spoiler" } }
        }))
        .expect("directives short-form deserialises");
        let features = cfg.features.expect("features present");
        let map = features.directives.expect("directives present");
        assert_eq!(
            map.get("spoiler"),
            Some(&DirectiveSpec::Short("Spoiler".to_string()))
        );
    }

    // `directives: { kbd: { component, kind, titleFromLabel } }` — full-form entry.
    #[test]
    fn directives_full_form_round_trip() {
        let cfg: MarkdownConfig = serde_json::from_value(serde_json::json!({
            "features": {
                "directives": {
                    "kbd": { "component": "Kbd", "kind": "text", "titleFromLabel": false }
                }
            }
        }))
        .expect("directives full-form deserialises");
        let features = cfg.features.expect("features present");
        let map = features.directives.expect("directives present");
        assert_eq!(
            map.get("kbd"),
            Some(&DirectiveSpec::Full(DirectiveFullSpec {
                component: "Kbd".to_string(),
                kind: Some(DirectiveSpecKind::Text),
                title_from_label: Some(false),
            }))
        );
    }

    // Untagged ordering: a directive value object parses as Full(...), a bare
    // string parses as Short(...).
    #[test]
    fn directive_spec_untagged_ordering() {
        let full: DirectiveSpec = serde_json::from_value(serde_json::json!({ "component": "Kbd" }))
            .expect("object deserialises");
        assert!(matches!(full, DirectiveSpec::Full(_)));
        let short: DirectiveSpec =
            serde_json::from_value(serde_json::json!("Spoiler")).expect("string deserialises");
        assert!(matches!(short, DirectiveSpec::Short(_)));
    }

    // `deny_unknown_fields` on `MarkdownFeaturesConfig` must reject a typo'd
    // feature key regardless of the new `directives` field.
    #[test]
    fn features_deny_unknown_fields_still_rejects_typo() {
        let err = serde_json::from_value::<MarkdownConfig>(serde_json::json!({
            "features": { "directivez": { "spoiler": "Spoiler" } }
        }))
        .expect_err("typo'd feature key must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("directivez") || msg.contains("unknown field") || msg.contains("denied"),
            "error must name the unknown field; got: {msg}"
        );
    }

    // `into_directive_def` conversion — Short form.
    #[test]
    fn into_directive_def_short_form() {
        use zfb_md_ast::{into_directive_def, DirectiveKind, DirectiveSpec};
        let spec = DirectiveSpec::Short("Spoiler".to_string());
        let def = into_directive_def("spoiler", &spec);
        assert_eq!(def.name, "spoiler");
        assert_eq!(def.component_name, "Spoiler");
        assert_eq!(def.kind, DirectiveKind::Container);
        assert!(def.title_from_label);
        assert!(def.attrs.is_empty());
    }

    // `into_directive_def` conversion — Full form with explicit kind + titleFromLabel.
    #[test]
    fn into_directive_def_full_form() {
        use zfb_md_ast::{
            into_directive_def, DirectiveFullSpec, DirectiveKind, DirectiveSpec, DirectiveSpecKind,
        };
        let spec = DirectiveSpec::Full(DirectiveFullSpec {
            component: "Kbd".to_string(),
            kind: Some(DirectiveSpecKind::Text),
            title_from_label: Some(false),
        });
        let def = into_directive_def("kbd", &spec);
        assert_eq!(def.name, "kbd");
        assert_eq!(def.component_name, "Kbd");
        assert_eq!(def.kind, DirectiveKind::Text);
        assert!(!def.title_from_label);
        assert!(def.attrs.is_empty());
    }

    // `into_directive_def` conversion — Full form with defaults (no kind, no titleFromLabel).
    #[test]
    fn into_directive_def_full_form_defaults() {
        use zfb_md_ast::{into_directive_def, DirectiveFullSpec, DirectiveKind, DirectiveSpec};
        let spec = DirectiveSpec::Full(DirectiveFullSpec {
            component: "MyBlock".to_string(),
            kind: None,
            title_from_label: None,
        });
        let def = into_directive_def("my-block", &spec);
        assert_eq!(def.kind, DirectiveKind::Container);
        assert!(def.title_from_label); // default true
    }
}
