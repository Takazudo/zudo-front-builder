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

use std::collections::{BTreeMap, HashSet};
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

    /// Whether production HTML pages should be minified after render.
    ///
    /// Default: `false` (off) for compatibility. `zfb build
    /// --minify-html` / `--no-minify-html` can override this value for a
    /// single build; the build command resolves that CLI tri-state before
    /// handing the config to orchestration, so downstream code only sees the
    /// effective boolean.
    ///
    /// Mirrors `ZfbConfig::minifyHtml` in `packages/zfb/src/config.ts`.
    #[serde(default)]
    pub minify_html: bool,

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
    /// SSR bundle into a deploy-ready entry (e.g. `dist/_worker.js` for
    /// Cloudflare Workers Static Assets, Pages-compatible).
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

    /// Config presets to merge before validation (#1196, #1199, #1202).
    ///
    /// Each preset is a partial `ZfbConfig`-shaped object. Presets are merged
    /// at the raw `serde_json::Value` layer BEFORE the user config is
    /// deserialized into `Config` (and before `validate()`), so the merge
    /// keys on key *presence* rather than value-equals-default:
    ///
    /// - **The four top-level additive array fields** (`plugins`,
    ///   `collections`, `extraWatchPaths`, `allowedHosts`): the merged value
    ///   is `[first preset…, second preset…, user…]` — earlier-declared
    ///   presets come first, the user's entries last. (Nested arrays like
    ///   `bundle.exclude` are NOT additive — user-wins-if-present.)
    /// - **Scalars / objects**: a key the user PROVIDED wins (even when the
    ///   value equals the type default — #1199); a key the user omitted is
    ///   filled from the first preset that supplies it. Objects recurse to
    ///   arbitrary depth (#1202), so a preset's nested sibling survives a
    ///   user value set elsewhere in the same nested object. An explicit
    ///   `null` (e.g. `adapter: null`) blocks the preset value (opt-out).
    ///
    /// `presets` is stripped before the final deserialize (and any nested
    /// `presets` key inside a preset is dropped — no recursive expansion),
    /// so downstream consumers never see it.
    ///
    /// The TS form is `presets: [somePreset()]` where `somePreset()`
    /// returns a `Partial<ZfbConfig>`. `#[serde(rename_all = "camelCase")]`
    /// deserialises `presets` 1:1.
    #[serde(default)]
    pub presets: Vec<serde_json::Value>,
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
            minify_html: false,
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
            presets: Vec::new(),
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
    /// Opt-in to a `path` that escapes the project root via `..`
    /// (e.g. a monorepo-shared content dir living outside this
    /// package). Default `false` keeps the standard
    /// [`ensure_path_in_root`] protection. Absolute paths and Windows
    /// drive-relative/prefix forms are rejected regardless of this
    /// flag — only `..`-relative escapes are relaxed. See `validate`.
    #[serde(default)]
    pub allow_outside_root: bool,
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

    /// Additional esbuild loaders keyed by file extension (for example
    /// `".txt" -> "text"`). Only inline loaders are accepted; loaders that
    /// emit sibling assets (`file` / `copy`) are rejected during config
    /// validation because the client pipelines do not publish those outputs.
    #[serde(default)]
    pub loaders: Option<BTreeMap<String, String>>,

    /// Operator-authored esbuild `--define` substitutions.
    ///
    /// Values are raw JavaScript expressions. A string value therefore needs
    /// to be pre-quoted JSON (for example `"\"production\""`). Reserved
    /// mode keys are rejected during config validation.
    #[serde(default)]
    pub define: Option<BTreeMap<String, String>>,
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

    /// Output mode for fenced-code highlighting (Highlight Tokens epic,
    /// zfb#1528). `"inline"` (default) bakes per-token colors into
    /// `style="color:#rrggbb"` (or the dual `--shiki-*` custom
    /// properties). `"class"` emits a semantic role class per token
    /// instead, so colors become re-themeable CSS design tokens rather
    /// than baked-in HTML.
    ///
    /// Mutually exclusive with [`Self::theme`] / [`Self::theme_light`] /
    /// [`Self::theme_dark`] / [`Self::themes_dir`] — themes don't affect
    /// class emission, so setting both is rejected rather than silently
    /// ignoring the theme.
    #[serde(default)]
    pub mode: CodeHighlightMode,

    /// Class-name prefix for class-mode role classes (e.g. the default
    /// `"hi-"` yields `hi-kw`, `hi-str`, ...). Must match
    /// `/^[A-Za-z][A-Za-z0-9_-]*$/`. Only meaningful when [`Self::mode`]
    /// is [`CodeHighlightMode::Class`].
    #[serde(default = "default_class_prefix")]
    pub class_prefix: String,

    /// Per-role class overrides for class mode, e.g.
    /// `{ "keyword": "text-violet-600 dark:text-violet-400" }` to map a
    /// role onto Tailwind utilities instead of the default
    /// `{classPrefix}{role}` class. Keys must be one of the fixed role
    /// names in [`CODE_HIGHLIGHT_ROLES`]; a value may hold multiple
    /// space-separated classes. `None` (the default) uses
    /// `{classPrefix}{role}` for every role.
    #[serde(default)]
    pub role_classes: Option<BTreeMap<String, String>>,

    /// Whether to inject the built-in `--zfb-hi-*` token stylesheet
    /// (`zfb-hi.css`) into the combined `styles.css` output. Default
    /// `true`. Only meaningful in class mode. Does NOT affect the
    /// content pipeline or its fingerprint — it is lowered separately
    /// into the CSS wiring config (Highlight Tokens epic sub #1533).
    #[serde(default = "default_true")]
    pub default_stylesheet: bool,
}

/// `codeHighlight.mode` — `"inline"` (default) bakes per-token colors into
/// `style="color:#rrggbb"` / `--shiki-*` custom properties; `"class"` emits
/// a semantic role class per token instead (Highlight Tokens epic,
/// zfb#1528), leaving color resolution to CSS.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CodeHighlightMode {
    /// Per-token inline colors (pre-epic behaviour). Default.
    #[default]
    Inline,
    /// Per-token semantic role classes; colors resolved via CSS custom
    /// properties or user-authored/Tailwind utilities instead of inline
    /// styles.
    Class,
}

pub(crate) fn default_class_prefix() -> String {
    zfb_content::syntect_highlight::DEFAULT_CLASS_HIGHLIGHT_PREFIX.to_string()
}

/// The fixed 18-role semantic taxonomy for class-mode syntax highlighting
/// (Highlight Tokens epic, zfb#1528). The scope->role classifier
/// (`hi_roles.rs`, zfb#1529) owns the canonical table; this retains the
/// config validation API as a public const slice.
pub const CODE_HIGHLIGHT_ROLES: &[&str] = &zfb_content::hi_roles::HiRole::FULL_NAMES;

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
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default)]
    pub options: serde_json::Value,
    /// Absolute module specifier (file URL) the plugin host will load
    /// via dynamic `import()`. Populated by the TS-load path; `None`
    /// for JSON-only configs and synthetic test configs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_module: Option<String>,
    /// Provenance marker: the npm package name of the preset that
    /// contributed this plugin, stamped at authoring time by
    /// `definePreset(sourcePackage, config)` (#1215). `None` for
    /// top-level (non-preset) plugins.
    ///
    /// The serde key is verbatim `source_package` (snake_case) — it must
    /// match the literal `definePreset` stamps onto each preset plugin
    /// object, and `PluginConfig` carries no `#[serde(rename_all)]`. The
    /// marker is per-plugin data: it rides the Value-layer preset array
    /// concat untouched (no merge-code change), then Rust resolves the
    /// package name to the preset's installed dir and anchors plugin
    /// resolution preset-dir-first (#1216).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_package: Option<String>,
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

/// Resolve `bundle.loaders` into a deterministic extension-to-loader map.
/// Empty when `bundle` or `bundle.loaders` is absent.
#[must_use]
pub fn resolve_bundle_loaders(bundle: Option<&BundleConfig>) -> BTreeMap<String, String> {
    match bundle {
        Some(b) => b.loaders.clone().unwrap_or_default(),
        None => BTreeMap::new(),
    }
}

/// Resolve `bundle.define` into a deterministic key-to-expression map.
/// Values remain raw esbuild expressions and are not JSON-encoded here.
#[must_use]
pub fn resolve_bundle_define(bundle: Option<&BundleConfig>) -> BTreeMap<String, String> {
    match bundle {
        Some(b) => b.define.clone().unwrap_or_default(),
        None => BTreeMap::new(),
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
        // Parse to a raw Value first so the preset merge can run at the Value
        // layer (#1199, #1202): key *presence* is observable there, so a user
        // who explicitly sets a scalar to its type default still beats a
        // preset, and the merge recurses to arbitrary depth. The syntax error
        // here keeps the line/column message.
        let mut user_value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            anyhow!(
                "{}: invalid config JSON at line {}, column {}: {}",
                json_path.display(),
                e.line(),
                e.column(),
                e
            )
        })?;
        // #1196 — split out `presets`. When there are none, deserialize from
        // the original `text` so a TYPE/schema error keeps the line/column
        // message (`from_value` on a merged Value loses position info). Only
        // the preset path needs the Value-layer merge.
        let presets =
            take_presets(&mut user_value).map_err(|e| anyhow!("{}: {}", json_path.display(), e))?;
        let mut cfg: Config = if let Some(presets) = presets {
            // Validate each preset as a `Config` fragment BEFORE merging so an
            // invalid preset field surfaces even when the user also sets that
            // key (all `Config` fields are `#[serde(default)]`, so a partial
            // fragment deserializes cleanly).
            for (i, preset_value) in presets.iter().enumerate() {
                serde_json::from_value::<Config>(preset_value.clone()).map_err(|e| {
                    anyhow!(
                        "{}: failed to parse presets[{i}] as a zfb config fragment: {}",
                        json_path.display(),
                        e
                    )
                })?;
            }
            let preset_defaults = build_preset_defaults(presets);
            let merged_value = merge_user_over_presets(preset_defaults, user_value);
            // A merged Value loses byte offsets, so a type error can't name a
            // line/column — `.with_context()` still names the file.
            serde_json::from_value(merged_value)
                .with_context(|| format!("{}: invalid zfb config", json_path.display()))?
        } else {
            serde_json::from_str(&text).map_err(|e| {
                anyhow!(
                    "{}: invalid config JSON at line {}, column {}: {}",
                    json_path.display(),
                    e.line(),
                    e.column(),
                    e
                )
            })?
        };
        // Issue #211: the JSON config path used to leave every
        // `PluginConfig.resolved_module` at `None`, which made the
        // downstream plugin-host filter silently drop ALL plugins
        // (see `commands::plugins::build_plugin_specs`). Mirror the
        // shape the TS-load path produces (a `file://` URL) for plugin
        // entries that name a path on disk; bare specifiers require Node
        // module resolution (not available on the JSON path) and stay
        // `None` with a user-facing warning so the surprise is visible.
        // This also resolves any plugins contributed by presets above.
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
    // JSON path resolves all entries in one pass; the caller cannot distinguish
    // preset-contributed entries from user entries at this point, so we use
    // `from_preset: false` and keep the generic recovery hint.
    resolve_plugin_modules_where(cfg, dir, false, |_| true)
}

/// Like [`resolve_json_plugin_modules`] but only resolves entries that still
/// have `resolved_module = None`. Used on the TS-load path after the
/// evaluator's zip-assignment has already filled in the original (top-level)
/// plugins; preset-contributed plugins are the remaining `None` entries.
fn resolve_unresolved_plugin_modules(cfg: &mut Config, dir: &Path) -> Result<()> {
    // Only entries with `resolved_module = None` at this point are preset-contributed
    // (the TS evaluator already resolved top-level plugins). Pass `from_preset: true`
    // so the error message names the preset origin and the directory tried.
    resolve_plugin_modules_where(cfg, dir, true, |entry| entry.resolved_module.is_none())
}

/// Resolve a single plugin entry against the **project root** `dir` and write
/// the resulting `file://` URL onto `entry.resolved_module`. This is today's
/// project-root resolution path verbatim, including T2's `from_preset` clearer
/// error wording — extracted so [`resolve_plugin_modules_where`] can reuse it
/// both for marker-less plugins and as the graceful-degradation fallback when a
/// preset package's own dir cannot be resolved (#1216).
fn resolve_plugin_at_project_root(
    entry: &mut PluginConfig,
    dir: &Path,
    from_preset: bool,
) -> Result<()> {
    let name = entry.name.as_str();
    match zfb_config_loader::resolve_plugin_path_to_file_url(name, dir)? {
        Some(url) => {
            entry.resolved_module = Some(url);
        }
        None => {
            // Bare specifier — resolve via oxc_resolver (issue #211 fix).
            // Hard error if the package is not installed so the user gets
            // a clear signal rather than silent plugin-drop.
            let file_url =
                zfb_config_loader::resolve_node_bare_specifier(name, dir).with_context(|| {
                    if from_preset {
                        format!(
                            "plugin {:?} (contributed by a preset): package not found \
                             in node_modules under {:?} — add the package to your \
                             project's dependencies and run `pnpm install`",
                            name, dir,
                        )
                    } else {
                        format!(
                            "plugin {:?}: package not found in node_modules \
                             (did you run `pnpm install`?)",
                            name
                        )
                    }
                })?;
            entry.resolved_module = Some(file_url);
        }
    }
    Ok(())
}

/// Core resolver: calls the path / bare-specifier resolution logic for every
/// plugin entry that satisfies `pred`. Extracted so
/// [`resolve_json_plugin_modules`] and [`resolve_unresolved_plugin_modules`]
/// share one implementation.
///
/// `from_preset` controls the wording of the error message emitted when a bare
/// specifier cannot be found in `node_modules`. Pass `true` on the
/// preset/unresolved pass (TS path after merge) so the user knows the missing
/// package was contributed by a preset and knows where to look to fix it.
///
/// Provenance-aware (#1216): an entry carrying a `source_package` marker (a
/// preset packaged as a real npm dependency, stamped by `definePreset`, #1215)
/// is resolved **preset-dir-first, project-root fallback**. An entry without a
/// marker keeps today's project-root resolution exactly.
fn resolve_plugin_modules_where(
    cfg: &mut Config,
    dir: &Path,
    from_preset: bool,
    pred: impl Fn(&PluginConfig) -> bool,
) -> Result<()> {
    for entry in cfg.plugins.iter_mut().filter(|e| pred(e)) {
        // Owned so the error closures below can capture it while
        // `resolve_plugin_at_project_root` takes `&mut entry`.
        let name = entry.name.clone();
        let name = name.as_str();

        // Provenance-aware path (#1216): a plugin contributed by a preset
        // packaged as a real npm dependency (`definePreset(sourcePackage, …)`,
        // #1215) carries the preset's own package name in `source_package`.
        // Such plugins (relative `./search.js`, or a non-hoisted bare dep of
        // the preset) must resolve against the **preset's installed dir
        // first**, falling back to the project root. Resolve the package name
        // to its dir via T1's `resolve_package_dir`, then anchor-resolve via
        // T1's two-anchor helper (preset dir preferred, project root fallback).
        if let Some(pkg) = entry.source_package.clone() {
            match zfb_config_loader::resolve_package_dir(&pkg, dir) {
                Ok(preset_dir) => {
                    let file_url =
                        zfb_config_loader::resolve_plugin_from_anchors(name, &preset_dir, dir)
                            .with_context(|| {
                                format!(
                                    "plugin {name:?} (contributed by preset {pkg:?}): not found \
                                     in the preset's package dir ({}) nor in node_modules under \
                                     {dir:?} — ensure the preset ships the plugin or add the \
                                     package to your project's dependencies and run `pnpm install`",
                                    preset_dir.display(),
                                )
                            })?;
                    entry.resolved_module = Some(file_url);
                }
                Err(pkg_dir_err) => {
                    // The preset package itself could not be resolved from the
                    // project root (e.g. the documented exports-hides-package.json
                    // edge). Degrade gracefully to today's project-root resolution
                    // rather than hard-failing on the package-dir step; only if
                    // THAT also fails do we surface the clearer error, extended
                    // with the preset package name and the package-dir failure.
                    resolve_plugin_at_project_root(entry, dir, from_preset).with_context(|| {
                        format!(
                            "plugin {name:?} (contributed by preset {pkg:?}): the preset's \
                                 own package dir could not be resolved ({pkg_dir_err:#}), and \
                                 resolution against the project root failed too"
                        )
                    })?;
                }
            }
            continue;
        }

        // No provenance marker — keep today's project-root resolution exactly,
        // including T2's clearer error on the `from_preset` path.
        resolve_plugin_at_project_root(entry, dir, from_preset)?;
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
    parse_loaded_config(loaded, ts_path, dir)
}

/// Deserialise the evaluated config value into [`Config`] and apply the
/// resolved plugin module specifiers (one `file://` URL per `plugins[]`
/// entry, in declaration order).
///
/// `dir` is the project root — needed to resolve plugin specifiers that
/// presets contribute (the TS evaluator only resolves top-level `config.plugins`;
/// preset-contributed plugins are resolved Rust-side via the same logic used
/// on the JSON-load path).
fn parse_loaded_config(
    loaded: zfb_config_loader::LoadedTsConfig,
    ts_path: &Path,
    dir: &Path,
) -> Result<Config> {
    let zfb_config_loader::LoadedTsConfig {
        config: mut value,
        resolved_plugins: resolved,
    } = loaded;

    // Zip the TS evaluator's resolved specifiers onto the RAW top-level
    // `plugins` Value array, BEFORE the preset merge prepends preset plugins
    // and shifts the indices (#1196, #1199, #1202). The count guard anchors
    // on the original top-level plugin count — preset-contributed plugins are
    // not in `resolved` (the evaluator only sees top-level `config.plugins`).
    // The key inserted is `resolved_module` (snake_case): `PluginConfig` has
    // no `#[serde(rename_all)]`, so its serde key is the field name verbatim
    // (see `PluginConfig` definition).
    let top_level_plugin_count = value
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    if !resolved.is_empty() && resolved.len() != top_level_plugin_count {
        bail!(
            "{}: plugin resolution count mismatch (config has {} plugins, loader resolved {}); \
             this indicates a bug in the TS config evaluator",
            ts_path.display(),
            top_level_plugin_count,
            resolved.len()
        );
    }
    if !resolved.is_empty() {
        if let Some(plugins) = value
            .get_mut("plugins")
            .and_then(serde_json::Value::as_array_mut)
        {
            for (entry, resolved_specifier) in plugins.iter_mut().zip(resolved) {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert(
                        "resolved_module".to_string(),
                        serde_json::Value::String(resolved_specifier),
                    );
                }
            }
        }
    }

    // #1196 — split out `presets`, validate each as a `Config` fragment for
    // diagnostics, build `preset_defaults`, then merge the (presets-stripped)
    // user Value over it. Preset plugins prepend here, after the zip above, so
    // the resolved top-level plugins keep their `resolved_module` and the
    // count guard saw the original indices.
    let mut had_presets = false;
    let presets = take_presets(&mut value).map_err(|e| {
        anyhow!(
            "{}: failed to parse the default export: {}",
            ts_path.display(),
            e
        )
    })?;
    let merged_value = if let Some(presets) = presets {
        had_presets = true;
        for (i, preset_value) in presets.iter().enumerate() {
            serde_json::from_value::<Config>(preset_value.clone()).map_err(|e| {
                anyhow!(
                    "{}: failed to parse presets[{i}] as a zfb config fragment: {}",
                    ts_path.display(),
                    e
                )
            })?;
        }
        let preset_defaults = build_preset_defaults(presets);
        merge_user_over_presets(preset_defaults, value)
    } else {
        // Still strip a `presets: []` / `presets: null` key before the final
        // deserialize so it never reaches `Config`.
        if let Some(map) = value.as_object_mut() {
            map.remove("presets");
        }
        value
    };

    // serde_path_to_error wraps the deserialize with the field path that
    // failed (e.g. `framework`), so the error names the offending key
    // instead of only echoing the raw JSON below (issue #1359). Its
    // `Display` impl already renders as `{path}: {inner}`.
    let mut config: Config =
        serde_path_to_error::deserialize(merged_value.clone()).map_err(|e| {
            anyhow!(
            "{}: failed to parse the default export as zfb config JSON: {}\n--- received ---\n{}",
            ts_path.display(),
            e,
            merged_value
        )
        })?;

    // Resolve any plugin entries that still have `resolved_module = None`
    // (i.e. those contributed by presets). Mirrors the JSON-load path
    // resolution so preset plugins load and run on the TS path too. Only
    // needed when presets actually contributed entries.
    if had_presets {
        resolve_unresolved_plugin_modules(&mut config, dir).with_context(|| {
            format!(
                "{}: resolving plugin paths contributed by presets",
                ts_path.display()
            )
        })?;
    }

    Ok(config)
}

/// Top-level config keys whose arrays are *additive* across presets and the
/// user config (#1196): the merged value is `[first preset…, second preset…,
/// user…]` rather than a single side winning. These are the camelCase JSON
/// keys (`Config` is `#[serde(rename_all = "camelCase")]`). The rule applies
/// ONLY at the top level — nested arrays (`bundle.exclude`,
/// `resolveMarkdownLinks.dirs`, …) are user-wins-if-present, not additive.
const ADDITIVE_TOP_LEVEL_ARRAY_KEYS: &[&str] =
    &["plugins", "collections", "extraWatchPaths", "allowedHosts"];

/// Presence-aware deep merge of two `serde_json::Value`s (#1199, #1202).
///
/// `low` is the lower-priority side, `over` the higher-priority side. The
/// merge is driven by key *presence*, not value-equals-default, which is what
/// lets a user who explicitly sets a scalar to its type default still beat a
/// preset (#1199):
///
/// - Both objects → recurse key-by-key to arbitrary depth (#1202). A key
///   present only in `over` wins; a key present only in `low` is kept; a key
///   in both recurses.
/// - Otherwise the *present* side wins as a whole leaf, with `over` winning a
///   collision. (An explicit `null` in `over` is "present" and so blocks the
///   `low` value — the user-opt-out semantics for `adapter: null` etc.)
///
/// Additive-array handling for the four top-level keys lives in
/// [`merge_object`] (its `top_level` branch); this recursive helper always
/// treats arrays as plain present-side-wins leaves, which is correct for the
/// nested arrays (`bundle.exclude`, `resolveMarkdownLinks.dirs`, …) it sees.
fn deep_merge(low: serde_json::Value, over: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (low, over) {
        (Value::Object(low_map), Value::Object(over_map)) => {
            Value::Object(merge_object(low_map, over_map, false))
        }
        // `over` is present (any non-recursable value, including `null`) and
        // therefore wins as a whole leaf.
        (_, over) => over,
    }
}

/// Merge two JSON objects. `over` keys win; keys only in `low` are kept; keys
/// in both recurse via [`deep_merge`]. When `top_level` is true, the four
/// [`ADDITIVE_TOP_LEVEL_ARRAY_KEYS`] are concatenated `low ++ over` instead of
/// `over` winning.
fn merge_object(
    mut low: serde_json::Map<String, serde_json::Value>,
    over: serde_json::Map<String, serde_json::Value>,
    top_level: bool,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::Value;
    for (key, over_val) in over {
        match low.remove(&key) {
            Some(low_val) => {
                let merged = if top_level && ADDITIVE_TOP_LEVEL_ARRAY_KEYS.contains(&key.as_str()) {
                    match (low_val, over_val) {
                        (Value::Array(mut a), Value::Array(b)) => {
                            a.extend(b);
                            Value::Array(a)
                        }
                        // Either side isn't an array → fall back to present-side-wins.
                        (_, over_val) => over_val,
                    }
                } else {
                    deep_merge(low_val, over_val)
                };
                low.insert(key, merged);
            }
            None => {
                low.insert(key, over_val);
            }
        }
    }
    low
}

/// Strip the `presets` key from a preset object — `presets` is never expanded
/// recursively, so a `presets` key nested inside a preset is dropped (#1196).
fn strip_presets(mut value: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(map) = &mut value {
        map.remove("presets");
    }
    value
}

/// Remove the top-level `presets` key from the user config Value and return
/// its entries when present and non-empty (#1196). The key is removed in place
/// so it never reaches the final `from_value::<Config>` (presets are merged,
/// not deserialized).
///
/// - absent or empty array → `Ok(None)` (nothing to merge)
/// - non-empty array → `Ok(Some(items))`
/// - any other value (e.g. the malformed TS shape `presets: somePreset()`
///   instead of `[somePreset()]`, or `presets: null`) → `Err` — a non-array
///   `presets` is a config error, not a silent no-op. The pre-#1196 typed
///   deserialization (`presets: Vec<Value>`) rejected these shapes; merging at
///   the Value layer must preserve that or the malformed config would load with
///   its presets silently dropped.
fn take_presets(value: &mut serde_json::Value) -> Result<Option<Vec<serde_json::Value>>, String> {
    let Some(map) = value.as_object_mut() else {
        return Ok(None);
    };
    let Some(presets) = map.remove("presets") else {
        return Ok(None);
    };
    match presets {
        serde_json::Value::Array(items) if items.is_empty() => Ok(None),
        serde_json::Value::Array(items) => Ok(Some(items)),
        other => Err(format!(
            "`presets` must be an array of config fragments, got {}",
            json_type_name(&other)
        )),
    }
}

/// Phase 1 of the two-phase preset fold (#1196): build `preset_defaults` by
/// folding the declared presets in DECLARED order. An already-folded
/// (earlier-declared) key wins over a later preset, and the four top-level
/// additive arrays append the next preset's items — so `presets: [a, b]`
/// yields scalars from `a` and additive arrays `[a…, b…]`.
///
/// Each preset is `presets`-stripped first so a nested `presets` key never
/// leaks into the fold.
fn build_preset_defaults(presets: Vec<serde_json::Value>) -> serde_json::Value {
    use serde_json::Value;
    let mut acc = Value::Object(serde_json::Map::new());
    for preset in presets {
        let preset = strip_presets(preset);
        acc = match (acc, preset) {
            (Value::Object(acc_map), Value::Object(preset_map)) => {
                Value::Object(fold_next_preset(acc_map, preset_map))
            }
            // A non-object preset is not a valid config fragment; the
            // per-preset `from_value::<Config>` diagnostic at the call site
            // catches that. Keep the accumulator unchanged here.
            (acc, _) => acc,
        };
    }
    acc
}

/// Fold one later preset (`next`) into the accumulator (`acc`, the
/// earlier-declared presets). Earlier presets win scalars/objects; the four
/// top-level additive arrays become `acc ++ next` (earlier-declared first).
fn fold_next_preset(
    acc: serde_json::Map<String, serde_json::Value>,
    next: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::Value;
    // For non-additive keys: `acc` (earlier) must win, so `acc` is the `over`
    // side. `merge_object(low=next, over=acc, top_level=true)` does that, but
    // its additive concat would be `next ++ acc`. Pre-empt the additive keys
    // by merging them ourselves in `acc ++ next` order, then strip them from
    // both maps before the generic merge.
    let mut acc = acc;
    let mut next = next;
    let mut additive: Vec<(String, Value)> = Vec::new();
    for key in ADDITIVE_TOP_LEVEL_ARRAY_KEYS {
        let a = acc.remove(*key);
        let n = next.remove(*key);
        match (a, n) {
            (Some(Value::Array(mut av)), Some(Value::Array(nv))) => {
                av.extend(nv);
                additive.push(((*key).to_string(), Value::Array(av)));
            }
            (Some(a), None) => additive.push(((*key).to_string(), a)),
            (None, Some(n)) => additive.push(((*key).to_string(), n)),
            // Non-array on either side → earlier (acc) wins as a leaf.
            (Some(a), Some(_)) => additive.push(((*key).to_string(), a)),
            (None, None) => {}
        }
    }
    // Generic non-additive merge: `acc` (earlier) wins.
    let mut merged = merge_object(next, acc, false);
    merged.extend(additive);
    merged
}

/// Phase 2 of the two-phase preset fold (#1196): merge the user config Value
/// OVER `preset_defaults`. User keys win; the four top-level additive arrays
/// become `preset_defaults ++ user` (= `[first preset…, second preset…,
/// user…]`). The user value is `presets`-stripped first (the `presets` key is
/// never deserialized into the final `Config`).
fn merge_user_over_presets(
    preset_defaults: serde_json::Value,
    user: serde_json::Value,
) -> serde_json::Value {
    use serde_json::Value;
    let user = strip_presets(user);
    match (preset_defaults, user) {
        (Value::Object(defaults_map), Value::Object(user_map)) => {
            Value::Object(merge_object(defaults_map, user_map, true))
        }
        // No preset defaults (or a degenerate non-object) → the user value
        // stands alone.
        (_, user) => user,
    }
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
        if c.allow_outside_root {
            // allowOutsideRoot relaxes only the `..`-escape check. Absolute
            // paths stay rejected, and `Path::is_absolute()` alone is
            // weaker than `ensure_path_in_root`'s guard: on Windows a
            // drive-relative path like `C:temp` (Prefix, no RootDir) is
            // NOT "absolute" per Rust's definition, so check explicitly
            // for a Prefix/RootDir component too.
            if c.path.is_absolute()
                || c.path
                    .components()
                    .any(|comp| matches!(comp, Component::Prefix(_) | Component::RootDir))
            {
                bail!(
                    "collection {:?}: path {:?} must be relative even with allowOutsideRoot ({})",
                    c.name,
                    c.path,
                    dir.display()
                );
            }
        } else {
            ensure_path_in_root(&c.path, dir)
                .with_context(|| format!("collection {:?}", c.name))?;
        }
    }
    if let Some(ch) = &cfg.code_highlight {
        if let Some(td) = &ch.themes_dir {
            ensure_path_in_root(td, dir).context("codeHighlight.themesDir")?;
        }
        // Highlight Tokens epic (zfb#1528): class mode is mutually
        // exclusive with every theme knob — themes don't affect class
        // emission, so setting both would silently no-op the theme
        // rather than error. Runs before the theme-pair checks below so
        // a class-mode config with a lone themeLight/themeDark is
        // reported against the mode, not the incomplete pair.
        if ch.mode == CodeHighlightMode::Class {
            if ch.theme.is_some() {
                bail!(
                    "codeHighlight.mode \"class\" is mutually exclusive with codeHighlight.theme"
                );
            }
            if ch.theme_light.is_some() {
                bail!(
                    "codeHighlight.mode \"class\" is mutually exclusive with \
                     codeHighlight.themeLight"
                );
            }
            if ch.theme_dark.is_some() {
                bail!(
                    "codeHighlight.mode \"class\" is mutually exclusive with \
                     codeHighlight.themeDark"
                );
            }
            if ch.themes_dir.is_some() {
                bail!(
                    "codeHighlight.mode \"class\" is mutually exclusive with \
                     codeHighlight.themesDir"
                );
            }
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
        let empty_role_classes = BTreeMap::new();
        let role_classes = ch.role_classes.as_ref().unwrap_or(&empty_role_classes);
        zfb_content::syntect_highlight::validate_class_highlight_classes(
            &ch.class_prefix,
            role_classes,
        )
        .map_err(|error| anyhow::anyhow!("codeHighlight.{error}"))?;
        if let Some(role_classes) = &ch.role_classes {
            // Authored-CSS path (`tailwind.enabled=false`): allowed, but no
            // safelist can be generated for these classes on that path, so
            // the mapped utilities must already exist in user-authored CSS.
            let tailwind_enabled = cfg.tailwind.as_ref().map(|t| t.enabled).unwrap_or(true);
            if !role_classes.is_empty() && !tailwind_enabled {
                tracing::warn!(
                    "codeHighlight.roleClasses is set with tailwind.enabled=false: no \
                     Tailwind safelist can be generated for these classes on the \
                     authored-CSS path — ensure the mapped utilities already exist in \
                     your own CSS"
                );
            }
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
    if let Some(bundle) = &cfg.bundle {
        if let Some(loaders) = &bundle.loaders {
            const RESERVED_EXTENSIONS: &[&str] = &[".css", ".module.css", ".mdx", ".md"];
            const INLINE_LOADERS: &[&str] =
                &["text", "json", "base64", "dataurl", "binary", "empty"];

            for (extension, loader) in loaders {
                if !extension.starts_with('.') {
                    bail!(
                        "bundle.loaders key {:?} must be a file extension starting with `.`",
                        extension
                    );
                }
                if RESERVED_EXTENSIONS.contains(&extension.as_str()) {
                    bail!(
                        "bundle.loaders key {:?} is reserved by zfb and cannot be overridden",
                        extension
                    );
                }
                if !INLINE_LOADERS.contains(&loader.as_str()) {
                    bail!(
                        "bundle.loaders key {:?} uses unsupported loader {:?}; inline-only v1 accepts: {}",
                        extension,
                        loader,
                        INLINE_LOADERS.join(", ")
                    );
                }
            }
        }

        if let Some(define) = &bundle.define {
            const RESERVED_DEFINE_KEYS: &[&str] = &[
                "import.meta.env.PROD",
                "import.meta.env.DEV",
                "process.env.NODE_ENV",
            ];
            for key in define.keys() {
                if RESERVED_DEFINE_KEYS.contains(&key.as_str()) {
                    bail!(
                        "bundle.define key {:?} is reserved by zfb's bundle mode and cannot be overridden",
                        key
                    );
                }
            }
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

    /// Drive the Value-layer preset merge exactly as the load paths do:
    /// build `preset_defaults` from the declared presets, merge the user
    /// Value over it, then deserialize the result into a typed [`Config`].
    /// Used by the `#1196` / `#1199` / `#1202` unit tests in place of the old
    /// `merge_preset_into(&mut cfg, preset, &baseline)` direct call.
    fn merge_presets_to_config(presets: Vec<serde_json::Value>, user: serde_json::Value) -> Config {
        let preset_defaults = build_preset_defaults(presets);
        let merged = merge_user_over_presets(preset_defaults, user);
        serde_json::from_value(merged).expect("merged preset config deserializes")
    }

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

    // ── Class-mode config tests (Highlight Tokens epic, zfb#1528 / #1530) ──

    /// Defaults: `mode` is `"inline"`, `classPrefix` is `"hi-"`,
    /// `roleClasses` is absent, `defaultStylesheet` is `true`.
    #[tokio::test]
    async fn code_highlight_class_mode_fields_default() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.code_highlight, None);

        // Same defaults apply when codeHighlight is present but empty.
        let tmp2 = TempDir::new().unwrap();
        tokio::fs::write(
            tmp2.path().join("zfb.config.json"),
            r#"{ "codeHighlight": {} }"#,
        )
        .await
        .unwrap();
        let cfg2 = load_from_dir(tmp2.path()).await.expect("load ok");
        let ch = cfg2.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.mode, CodeHighlightMode::Inline);
        assert_eq!(ch.class_prefix, "hi-");
        assert_eq!(ch.role_classes, None);
        assert!(ch.default_stylesheet);
    }

    /// A valid class-mode config (mode + classPrefix + roleClasses +
    /// defaultStylesheet) parses cleanly.
    #[tokio::test]
    async fn code_highlight_valid_class_mode_config_parses() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "codeHighlight": {
                    "mode": "class",
                    "classPrefix": "syn-",
                    "roleClasses": { "keyword": "text-violet-600 dark:text-violet-400" },
                    "defaultStylesheet": false
                }
            }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.mode, CodeHighlightMode::Class);
        assert_eq!(ch.class_prefix, "syn-");
        assert_eq!(
            ch.role_classes.as_ref().and_then(|m| m.get("keyword")),
            Some(&"text-violet-600 dark:text-violet-400".to_string())
        );
        assert!(!ch.default_stylesheet);
    }

    /// `mode:"class"` + `theme` must be rejected, naming both fields.
    #[tokio::test]
    async fn code_highlight_class_mode_and_theme_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "mode": "class", "theme": "InspiredGitHub" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("mode class + theme must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.mode") && msg.contains("codeHighlight.theme"),
            "error must name both codeHighlight.mode and codeHighlight.theme; got: {msg}"
        );
    }

    /// `mode:"class"` + `themeLight` must be rejected, naming both fields.
    #[tokio::test]
    async fn code_highlight_class_mode_and_theme_light_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "mode": "class", "themeLight": "base16-ocean.light" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("mode class + themeLight must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.mode") && msg.contains("codeHighlight.themeLight"),
            "error must name both codeHighlight.mode and codeHighlight.themeLight; got: {msg}"
        );
    }

    /// `mode:"class"` + `themeDark` must be rejected, naming both fields.
    #[tokio::test]
    async fn code_highlight_class_mode_and_theme_dark_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "mode": "class", "themeDark": "base16-ocean.dark" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("mode class + themeDark must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.mode") && msg.contains("codeHighlight.themeDark"),
            "error must name both codeHighlight.mode and codeHighlight.themeDark; got: {msg}"
        );
    }

    /// `mode:"class"` + `themesDir` must be rejected, naming both fields.
    #[tokio::test]
    async fn code_highlight_class_mode_and_themes_dir_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "mode": "class", "themesDir": "themes" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("mode class + themesDir must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.mode") && msg.contains("codeHighlight.themesDir"),
            "error must name both codeHighlight.mode and codeHighlight.themesDir; got: {msg}"
        );
    }

    /// An unrecognised `mode` value is rejected by serde with a clear
    /// "unknown variant" style error (no custom validation needed).
    #[tokio::test]
    async fn code_highlight_unknown_mode_value_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "mode": "block" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("unknown mode value must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown variant `block`")
                && msg.contains("`inline`")
                && msg.contains("`class`"),
            "error must reject the bad value and list the valid variants; got: {msg}"
        );
    }

    /// An unknown `roleClasses` key is rejected, naming the bad key and
    /// listing the valid roles.
    #[tokio::test]
    async fn code_highlight_role_classes_unknown_key_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "codeHighlight": {
                    "mode": "class",
                    "roleClasses": { "not-a-role": "hi-foo" }
                }
            }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("unknown roleClasses key must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not-a-role"),
            "error must name the bad key; got: {msg}"
        );
        assert!(
            msg.contains("keyword") && msg.contains("string"),
            "error must list the valid roles; got: {msg}"
        );
    }

    /// Every one of the 18 fixed role names is accepted as a `roleClasses`
    /// key.
    #[tokio::test]
    async fn code_highlight_role_classes_every_known_role_accepted() {
        let entries: String = CODE_HIGHLIGHT_ROLES
            .iter()
            .map(|role| format!(r#""{role}": "hi-{role}""#))
            .collect::<Vec<_>>()
            .join(",");
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            format!(
                r#"{{ "codeHighlight": {{ "mode": "class", "roleClasses": {{ {entries} }} }} }}"#
            ),
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("all 18 roles must be accepted");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(
            ch.role_classes.as_ref().map(|m| m.len()),
            Some(CODE_HIGHLIGHT_ROLES.len())
        );
    }

    /// A `roleClasses` value containing the bare token `"line"` is rejected
    /// (collides with the code-enrichment line wrapper class).
    #[tokio::test]
    async fn code_highlight_role_classes_line_token_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "codeHighlight": {
                    "mode": "class",
                    "roleClasses": { "keyword": "hi-kw line" }
                }
            }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("roleClasses value containing the \"line\" token must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("\"line\""),
            "error must mention the offending \"line\" token; got: {msg}"
        );
    }

    /// A `roleClasses` value that merely CONTAINS "line" as a substring of
    /// a longer class name (not a standalone token) is fine.
    #[tokio::test]
    async fn code_highlight_role_classes_line_substring_is_allowed() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "codeHighlight": {
                    "mode": "class",
                    "roleClasses": { "keyword": "underline" }
                }
            }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("a class name that merely contains \"line\" as a substring is fine");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(
            ch.role_classes.as_ref().and_then(|m| m.get("keyword")),
            Some(&"underline".to_string())
        );
    }

    /// An empty `classPrefix` is rejected.
    #[tokio::test]
    async fn code_highlight_class_prefix_empty_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "classPrefix": "" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("empty classPrefix must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.classPrefix"),
            "error should mention field; got: {msg}"
        );
    }

    /// A `classPrefix` starting with a digit is rejected (must start with
    /// an ASCII letter).
    #[tokio::test]
    async fn code_highlight_class_prefix_leading_digit_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "classPrefix": "1hi-" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("classPrefix starting with a digit must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.classPrefix"),
            "error should mention field; got: {msg}"
        );
    }

    /// A `classPrefix` containing a disallowed character (e.g. a dot) is
    /// rejected.
    #[tokio::test]
    async fn code_highlight_class_prefix_invalid_character_rejected() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "classPrefix": "hi.foo" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("classPrefix with a disallowed character must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("codeHighlight.classPrefix"),
            "error should mention field; got: {msg}"
        );
    }

    /// A `classPrefix` using underscores and hyphens throughout the tail is
    /// accepted (matches the documented pattern).
    #[tokio::test]
    async fn code_highlight_class_prefix_underscores_and_hyphens_accepted() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "codeHighlight": { "classPrefix": "Hi_Token-" } }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("classPrefix with underscores/hyphens must be accepted");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(ch.class_prefix, "Hi_Token-");
    }

    /// `roleClasses` set while `tailwind.enabled` is `false` (the
    /// authored-CSS path) is ALLOWED — not an error — even though no
    /// Tailwind safelist can be generated for those classes on that path.
    #[tokio::test]
    async fn code_highlight_role_classes_with_tailwind_disabled_is_allowed() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "tailwind": { "enabled": false },
                "codeHighlight": {
                    "mode": "class",
                    "roleClasses": { "keyword": "my-keyword-class" }
                }
            }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("roleClasses + tailwind.enabled=false must be allowed (warning only)");
        let ch = cfg.code_highlight.as_ref().expect("codeHighlight present");
        assert_eq!(
            ch.role_classes.as_ref().and_then(|m| m.get("keyword")),
            Some(&"my-keyword-class".to_string())
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
        assert!(
            !cfg.minify_html,
            "omitted minifyHtml must default to compatibility-off"
        );
    }

    #[tokio::test]
    async fn loads_minify_html_true_from_camelcase_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "minifyHtml": true }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert!(cfg.minify_html);
    }

    #[tokio::test]
    async fn loads_minify_html_false_from_camelcase_json() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "minifyHtml": false }"#,
        )
        .await
        .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert!(!cfg.minify_html);
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

    // ── allowOutsideRoot opt-in (#1549) ───────────────────────────────────

    #[tokio::test]
    async fn allow_outside_root_permits_parent_dir_escape() {
        // Flag-SET twin of `rejects_parent_dir_escape` above: with
        // `allowOutsideRoot: true`, a `..`-relative collection path is
        // now accepted.
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "../outside", "allowOutsideRoot": true }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("allowOutsideRoot should permit a `..` escape");
        assert_eq!(cfg.collections.len(), 1);
        assert!(cfg.collections[0].allow_outside_root);
    }

    #[tokio::test]
    async fn allow_outside_root_still_rejects_absolute_path() {
        // Absolute paths stay rejected even with the opt-in flag — only
        // `..`-relative escapes are relaxed.
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "/etc/passwd", "allowOutsideRoot": true }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("absolute path must stay rejected even with allowOutsideRoot");
        let msg = format!("{err:#}");
        assert!(msg.contains("relative"), "msg: {msg}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn allow_outside_root_still_rejects_windows_prefix_path() {
        // `Component::Prefix` only exists when compiled for a Windows
        // target, so this guard (and this test) only bites on Windows —
        // zfb ships a windows binary (`@takazudo/zfb-win32-x64-msvc`).
        // `C:temp` is drive-relative (Prefix component, no RootDir) and
        // is NOT "absolute" per `Path::is_absolute()`'s own definition,
        // so the opt-in branch checks for a Prefix/RootDir component
        // explicitly — `is_absolute()` alone would let it through.
        for bad_path in [r"C:temp", r"C:\foo", r"\\server\share"] {
            let tmp = TempDir::new().unwrap();
            let json = format!(
                r#"{{ "collections": [ {{ "name": "blog", "path": {bad_path:?}, "allowOutsideRoot": true }} ] }}"#
            );
            tokio::fs::write(tmp.path().join("zfb.config.json"), json)
                .await
                .unwrap();
            let err = load_from_dir(tmp.path()).await.unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("relative"), "path {bad_path:?}, msg: {msg}");
        }
    }

    #[tokio::test]
    async fn allow_outside_root_unset_keeps_rejecting_parent_dir_escape() {
        // Explicit `allowOutsideRoot: false` must behave identically to
        // the flag being absent (already covered by
        // `rejects_parent_dir_escape`).
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "../outside", "allowOutsideRoot": false }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject .. escape when allowOutsideRoot is explicitly false");
        let msg = format!("{err:#}");
        assert!(msg.contains("escapes"), "msg: {msg}");
    }

    #[tokio::test]
    async fn resolve_markdown_links_docs_dir_dotdot_escape_rejected() {
        // Direct regression: the allowOutsideRoot bypass lives at the
        // collection call site only, so `resolveMarkdownLinks.docsDir`
        // must keep rejecting `..` escapes unconditionally.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "resolveMarkdownLinks": { "docsDir": "../outside-docs" } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("must reject .. escape");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("resolveMarkdownLinks.docsDir"),
            "error should mention field; got: {msg}"
        );
        assert!(msg.contains("escapes"), "msg: {msg}");
    }

    #[tokio::test]
    async fn resolve_markdown_links_dirs_entry_dotdot_escape_rejected() {
        // Direct regression: `resolveMarkdownLinks.dirs[].dir` has no
        // opt-in bypass either.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{ "resolveMarkdownLinks": { "dirs": [ { "dir": "../outside-docs", "routePrefix": "/docs/" } ] } }"#,
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("must reject .. escape");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("resolveMarkdownLinks.dirs[0].dir"),
            "error should mention field; got: {msg}"
        );
        assert!(msg.contains("escapes"), "msg: {msg}");
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

    // --- bundle.loaders / bundle.define tests (#1498) ---------------------

    #[test]
    fn bundle_loaders_and_define_deserialise_and_resolve_verbatim() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "bundle": {
                "loaders": {
                    ".txt": "text",
                    ".bin": "binary"
                },
                "define": {
                    "__APP_NAME__": "\"zfb\"",
                    "__FEATURE_ENABLED__": "true"
                }
            }
        }))
        .expect("bundle.loaders / bundle.define deserialise");

        assert_eq!(
            resolve_bundle_loaders(cfg.bundle.as_ref()),
            BTreeMap::from([
                (".bin".to_string(), "binary".to_string()),
                (".txt".to_string(), "text".to_string()),
            ])
        );
        assert_eq!(
            resolve_bundle_define(cfg.bundle.as_ref()),
            BTreeMap::from([
                ("__APP_NAME__".to_string(), "\"zfb\"".to_string()),
                ("__FEATURE_ENABLED__".to_string(), "true".to_string()),
            ])
        );
        validate(&cfg, Path::new(".")).expect("valid bundle knobs pass validation");
    }

    #[test]
    fn bundle_absent_resolves_to_empty_loaders_and_define() {
        let cfg: Config =
            serde_json::from_value(serde_json::json!({})).expect("empty config deserialises");
        assert!(resolve_bundle_loaders(cfg.bundle.as_ref()).is_empty());
        assert!(resolve_bundle_define(cfg.bundle.as_ref()).is_empty());

        let cfg: Config = serde_json::from_value(serde_json::json!({ "bundle": {} }))
            .expect("bundle:{} deserialises");
        assert!(resolve_bundle_loaders(cfg.bundle.as_ref()).is_empty());
        assert!(resolve_bundle_define(cfg.bundle.as_ref()).is_empty());
    }

    #[test]
    fn bundle_loaders_reject_reserved_extensions_and_name_the_key() {
        for extension in [".css", ".module.css", ".mdx", ".md"] {
            let cfg: Config = serde_json::from_value(serde_json::json!({
                "bundle": { "loaders": { (extension): "text" } }
            }))
            .expect("config shape deserialises before semantic validation");
            let err = validate(&cfg, Path::new("."))
                .expect_err("reserved loader extension must fail validation");
            let message = format!("{err:#}");
            assert!(
                message.contains(extension),
                "error must name key: {message}"
            );
            assert!(
                message.contains("reserved"),
                "error must explain: {message}"
            );
        }
    }

    #[test]
    fn bundle_loaders_reject_asset_emitting_or_unknown_loaders() {
        for loader in ["file", "copy", "css"] {
            let cfg: Config = serde_json::from_value(serde_json::json!({
                "bundle": { "loaders": { ".asset": loader } }
            }))
            .expect("config shape deserialises before semantic validation");
            let err =
                validate(&cfg, Path::new(".")).expect_err("non-inline loader must fail validation");
            let message = format!("{err:#}");
            assert!(message.contains(".asset"), "error must name key: {message}");
            assert!(
                message.contains(loader),
                "error must name loader: {message}"
            );
            assert!(
                message.contains("inline-only"),
                "error must explain: {message}"
            );
        }
    }

    #[test]
    fn bundle_loaders_reject_keys_without_a_leading_dot() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "bundle": { "loaders": { "txt": "text" } }
        }))
        .expect("config shape deserialises before semantic validation");
        let err = validate(&cfg, Path::new("."))
            .expect_err("loader extension without dot must fail validation");
        let message = format!("{err:#}");
        assert!(message.contains("txt"), "error must name key: {message}");
        assert!(
            message.contains("starting with `.`"),
            "error must explain: {message}"
        );
    }

    #[test]
    fn bundle_define_rejects_reserved_mode_keys_and_names_them() {
        for key in [
            "import.meta.env.PROD",
            "import.meta.env.DEV",
            "process.env.NODE_ENV",
        ] {
            let cfg: Config = serde_json::from_value(serde_json::json!({
                "bundle": { "define": { (key): "false" } }
            }))
            .expect("config shape deserialises before semantic validation");
            let err = validate(&cfg, Path::new("."))
                .expect_err("reserved define key must fail validation");
            let message = format!("{err:#}");
            assert!(message.contains(key), "error must name key: {message}");
            assert!(
                message.contains("reserved"),
                "error must explain: {message}"
            );
        }
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

    /// Real-pipeline error-quality test for a `zfb.config.ts` with a wrong
    /// enum value (issue #1353 / #1359). Relocated here from
    /// `zfb-render/tests/error_messages.rs` — see that file's pointer comment
    /// for why the real pipeline is only reachable from this crate.
    ///
    /// Goes through the FULL real pipeline: embedded esbuild bundles the
    /// file, the in-process V8 isolate evaluates it, and
    /// `serde_path_to_error` (adopted by this issue) turns the resulting
    /// `serde_json` error into a field-path diagnostic. No
    /// `test_default_export_json` mock — this is Level 3 (executes the
    /// emitted bundle in real V8), not a logic-only test.
    ///
    /// `framework` is deliberately given a WRONG VALUE, not omitted:
    /// `Config.framework` is `#[serde(default)]` (defaults to `Preact`), so a
    /// missing field loads cleanly and could never make this test fail.
    #[cfg(feature = "embed_v8")]
    #[tokio::test]
    async fn invalid_zfb_config_ts_points_at_field_and_file() {
        let tmp = TempDir::new().unwrap();
        let ts_path = tmp.path().join("zfb.config.ts");
        tokio::fs::write(&ts_path, "export default { framework: \"vue\" };\n")
            .await
            .unwrap();

        let err = load_from_dir(tmp.path())
            .await
            .expect_err("unknown framework variant should be rejected");
        let msg = format!("{err:#}");

        assert!(
            msg.contains(ts_path.to_str().unwrap()),
            "error should name the absolute zfb.config.ts path: {msg}"
        );
        // The `framework: unknown variant` shape is serde_path_to_error's
        // `{path}: {inner}` rendering — a bare `contains("framework")` would
        // also pass via the `--- received ---` JSON echo, which is exactly
        // the failure mode this test exists to rule out.
        assert!(
            msg.contains("framework: unknown variant"),
            "error should name the bad field via its serde path: {msg}"
        );
        assert!(
            msg.contains("preact") && msg.contains("react"),
            "error should list the expected union values: {msg}"
        );
    }

    /// Second real-pipeline error-quality case (issue #1359): a collection
    /// entry missing its required `path` field. `CollectionDef.path` has no
    /// `#[serde(default)]`, so — unlike `framework` — omitting it is a
    /// genuine schema error, letting this case exercise the missing-field
    /// branch of `serde_path_to_error` rather than the unknown-variant
    /// branch the sibling test above covers.
    #[cfg(feature = "embed_v8")]
    #[tokio::test]
    async fn invalid_zfb_config_ts_collection_missing_path_field() {
        let tmp = TempDir::new().unwrap();
        let ts_path = tmp.path().join("zfb.config.ts");
        tokio::fs::write(
            &ts_path,
            "export default { collections: [{ name: \"blog\" }] };\n",
        )
        .await
        .unwrap();

        let err = load_from_dir(tmp.path())
            .await
            .expect_err("collection missing `path` should be rejected");
        let msg = format!("{err:#}");

        assert!(
            msg.contains(ts_path.to_str().unwrap()),
            "error should name the absolute zfb.config.ts path: {msg}"
        );
        // `collections[0]` is serde_path_to_error's indexed path to the
        // offending entry — the JSON echo renders the array without indices,
        // so this substring can only come from the path diagnostic.
        assert!(
            msg.contains("collections[0]"),
            "error should point at the offending collection entry: {msg}"
        );
        assert!(
            msg.contains("missing field `path`"),
            "error should name the missing field: {msg}"
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

    // --- Preset merge tests (#1196) -------------------------------------------

    #[test]
    fn preset_plugins_prepend_to_main_plugins() {
        // A preset contributes a plugin; the user config has its own plugin.
        // The preset plugin should appear BEFORE the user plugin (additive
        // top-level array: `[preset…, user…]`).
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "plugins": [{ "name": "preset-plugin", "options": {} }]
            })],
            serde_json::json!({
                "plugins": [{ "name": "user-plugin", "options": {} }]
            }),
        );
        assert_eq!(cfg.plugins.len(), 2);
        assert_eq!(cfg.plugins[0].name, "preset-plugin");
        assert_eq!(cfg.plugins[1].name, "user-plugin");
    }

    #[test]
    fn preset_scalar_fills_default_only() {
        // A preset sets `adapter`; the user config omits it. The preset value
        // fills in.
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "adapter": "@takazudo/zfb-adapter-cloudflare" })],
            serde_json::json!({}),
        );
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("@takazudo/zfb-adapter-cloudflare")
        );
    }

    #[test]
    fn main_config_wins_over_preset_scalar() {
        // The user config already sets `adapter`; the preset must NOT override
        // it (user-wins-if-present).
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "adapter": "preset-adapter" })],
            serde_json::json!({ "adapter": "user-adapter" }),
        );
        assert_eq!(cfg.adapter.as_deref(), Some("user-adapter"));
    }

    #[test]
    fn preset_collections_prepend() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "collections": [{ "name": "preset-coll", "path": "content/preset" }]
            })],
            serde_json::json!({
                "collections": [{ "name": "user-coll", "path": "content/user" }]
            }),
        );
        assert_eq!(cfg.collections.len(), 2);
        assert_eq!(cfg.collections[0].name, "preset-coll");
        assert_eq!(cfg.collections[1].name, "user-coll");
    }

    #[test]
    fn preset_deserialization_from_json_value() {
        // Verify the end-to-end serde roundtrip: a `serde_json::Value`
        // carrying a preset can be deserialized into `Config` (which is
        // what `parse_loaded_config` does for each preset entry).
        let preset_value = serde_json::json!({
            "plugins": [{ "name": "my-preset-plugin", "options": {} }],
            "adapter": "@takazudo/zfb-adapter-cloudflare",
        });
        let preset: Config = serde_json::from_value(preset_value).unwrap();
        assert_eq!(preset.plugins.len(), 1);
        assert_eq!(preset.plugins[0].name, "my-preset-plugin");
        assert_eq!(
            preset.adapter.as_deref(),
            Some("@takazudo/zfb-adapter-cloudflare")
        );
    }

    // --- Preset plugin resolution tests (#1196 Bug 1 fix) ---------------------

    /// TS-path preset plugin gets `resolved_module` populated Rust-side.
    ///
    /// Regression guard for Bug 1 / Bug 1-residual: the TS evaluator only
    /// resolves top-level `config.plugins`; preset-contributed plugins must
    /// be resolved by the Rust side after the preset merge, or they would be
    /// silently dropped by the plugin-host filter.
    #[tokio::test]
    async fn ts_path_preset_plugin_is_resolved() {
        let tmp = TempDir::new().unwrap();

        // Create the plugin file so the path resolver can find it.
        let plugin_path = tmp.path().join("preset-plugin.mjs");
        tokio::fs::write(&plugin_path, "export default {};\n")
            .await
            .unwrap();

        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();

        // The envelope the TS evaluator (mocked) returns: top-level config has
        // no plugins of its own; the preset contributes one via `./preset-plugin.mjs`.
        // The `plugins: []` key in the envelope only lists top-level plugin
        // resolutions — the preset plugin is not listed there.
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "plugins": [{ "name": "./preset-plugin.mjs" }] }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };

        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("preset plugin on TS path should resolve without error");

        assert_eq!(
            cfg.plugins.len(),
            1,
            "preset plugin must appear in merged config"
        );
        assert_eq!(cfg.plugins[0].name, "./preset-plugin.mjs");
        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("preset plugin must have resolved_module populated (not None)");
        assert!(
            resolved.starts_with("file://"),
            "resolved_module must be a file:// URL, got {resolved:?}"
        );
    }

    /// JSON-path preset plugin gets `resolved_module` populated.
    ///
    /// Regression guard for Bug 2: `presets[]` in `zfb.config.json` were
    /// previously parsed but never merged — their plugins were silently discarded.
    #[tokio::test]
    async fn json_path_preset_plugin_is_resolved() {
        let tmp = TempDir::new().unwrap();

        // Create the plugin file so the path resolver can find it.
        let plugin_path = tmp.path().join("preset-plugin.mjs");
        tokio::fs::write(&plugin_path, "export default {};\n")
            .await
            .unwrap();

        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "presets": [
                    { "plugins": [{ "name": "./preset-plugin.mjs" }] }
                ]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("preset plugin on JSON path should resolve without error");

        assert_eq!(
            cfg.plugins.len(),
            1,
            "preset plugin must appear in merged config"
        );
        assert_eq!(cfg.plugins[0].name, "./preset-plugin.mjs");
        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("preset plugin must have resolved_module populated (not None)");
        assert!(
            resolved.starts_with("file://"),
            "resolved_module must be a file:// URL, got {resolved:?}"
        );
        // Verify the resolved URL actually points at our plugin file.
        let parsed = url::Url::parse(resolved).expect("valid url");
        let parsed_path = parsed.to_file_path().expect("file:// URL round-trips");
        assert_eq!(parsed_path, plugin_path.canonicalize().unwrap());
    }

    /// Error message for an unresolvable preset-contributed plugin names the
    /// specifier, flags that it came from a preset, and includes the project
    /// root directory that was searched (#1214).
    ///
    /// Uses the TS path (mock envelope) so the plugin goes through
    /// `resolve_unresolved_plugin_modules` (the preset/unresolved pass).
    #[tokio::test]
    async fn ts_path_unresolvable_preset_plugin_names_preset_in_error() {
        let tmp = TempDir::new().unwrap();

        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();

        // The preset contributes a bare specifier that is not installed —
        // `not-a-real-package-xyz` does not exist in node_modules.
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "plugins": [{ "name": "not-a-real-package-xyz" }] }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };

        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("loading a preset plugin whose package is not installed must fail");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("contributed by a preset"),
            "error must say the plugin came from a preset; got:\n{msg}"
        );
        assert!(
            msg.contains("not-a-real-package-xyz"),
            "error must name the plugin specifier; got:\n{msg}"
        );
        assert!(
            msg.contains("pnpm install"),
            "error must include the recovery hint; got:\n{msg}"
        );
    }

    // --- T4 provenance marker (#1216) ----------------------------------------

    /// Step 2: the `source_package` provenance marker is per-plugin data, so it
    /// must ride the Value-layer preset array concat untouched — through
    /// `build_preset_defaults` / `fold_next_preset` / `merge_object` (additive
    /// concat) and the final `from_value::<Config>` — with NO merge-code change.
    /// `Config` has no `deny_unknown_fields`; once the field exists it
    /// round-trips cleanly.
    #[test]
    fn source_package_marker_survives_value_layer_preset_merge() {
        // Two presets each contribute a marked plugin; the user adds an
        // unmarked plugin. The additive concat must yield
        // [presetA(marked), presetB(marked), user(unmarked)] with every
        // marker preserved verbatim.
        let presets = vec![
            serde_json::json!({
                "plugins": [
                    { "name": "./a.mjs", "source_package": "@scope/zfb-preset-a" }
                ]
            }),
            serde_json::json!({
                "plugins": [
                    { "name": "b-bare", "source_package": "@scope/zfb-preset-b" }
                ]
            }),
        ];
        let user = serde_json::json!({
            "plugins": [{ "name": "./user.mjs" }]
        });

        let cfg = merge_presets_to_config(presets, user);

        assert_eq!(cfg.plugins.len(), 3, "additive concat keeps all three");
        // Declared order: presetA, presetB, then user.
        assert_eq!(cfg.plugins[0].name, "./a.mjs");
        assert_eq!(
            cfg.plugins[0].source_package.as_deref(),
            Some("@scope/zfb-preset-a"),
            "preset A marker must survive the merge untouched"
        );
        assert_eq!(cfg.plugins[1].name, "b-bare");
        assert_eq!(
            cfg.plugins[1].source_package.as_deref(),
            Some("@scope/zfb-preset-b"),
            "preset B marker must survive the merge untouched"
        );
        assert_eq!(cfg.plugins[2].name, "./user.mjs");
        assert_eq!(
            cfg.plugins[2].source_package, None,
            "a top-level (non-preset) plugin carries no marker"
        );
    }

    /// Step 3: regression guard for the zip-by-index + count guard
    /// (parse_loaded_config, ~1653-1665). A config with top-level plugins AND a
    /// preset contributing plugins must resolve without tripping the
    /// count-mismatch bail. The new `source_package` field is
    /// `skip_serializing_if` and only ever set on preset (post-zip) plugins —
    /// which are not in `resolved` and prepend after the zip — so the guard
    /// keying on `top_level_plugin_count` is unaffected.
    #[tokio::test]
    async fn count_guard_holds_with_presets_present() {
        let tmp = TempDir::new().unwrap();
        // Top-level plugin file + preset plugin file both present at project root.
        for name in ["top.mjs", "from-preset.mjs"] {
            tokio::fs::write(tmp.path().join(name), "export default {};\n")
                .await
                .unwrap();
        }
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();

        // Envelope: ONE top-level plugin (so `plugins: [<one resolved url>]`),
        // plus a preset contributing one more. The count guard anchors on the
        // ONE top-level plugin; the preset plugin prepends after the zip and is
        // not in `resolved`.
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "plugins": [{ "name": "./top.mjs" }],
                        "presets": [
                            { "plugins": [{ "name": "./from-preset.mjs" }] }
                        ]
                    },
                    "plugins": ["file:///already/resolved/top.mjs"]
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };

        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("top-level + preset plugins must resolve without count-mismatch bail");

        // Declared order after additive prepend: [preset…, top-level…].
        assert_eq!(cfg.plugins.len(), 2, "both plugins present, none dropped");
        assert_eq!(cfg.plugins[0].name, "./from-preset.mjs");
        assert_eq!(cfg.plugins[1].name, "./top.mjs");
        // The top-level plugin kept the evaluator-zipped resolution; the preset
        // plugin was resolved Rust-side.
        assert_eq!(
            cfg.plugins[1].resolved_module.as_deref(),
            Some("file:///already/resolved/top.mjs"),
            "top-level plugin keeps its zipped resolved_module"
        );
        assert!(
            cfg.plugins[0]
                .resolved_module
                .as_deref()
                .is_some_and(|u| u.starts_with("file://")),
            "preset plugin gets resolved Rust-side"
        );
    }

    /// Build a minimal `node_modules/` tree mirroring the T1
    /// `node-modules-nested` fixture, inline, so this crate's tests can prove
    /// the provenance marker drives preset-dir-first resolution end-to-end.
    ///
    /// Layout under `<root>`:
    /// ```text
    /// node_modules/@scope/zfb-preset-example/
    ///     package.json (exports "." conditional + "./package.json")
    ///     search.js                         ← relative plugin inside preset
    ///     dist/index.mjs
    ///     node_modules/zfb-plugin-nested/    ← bare dep nested in the preset
    ///         package.json, dist/index.mjs
    /// ```
    /// Returns the project root (the tmp dir). Caller keeps `TempDir` alive.
    fn build_inline_preset_node_modules(root: &std::path::Path) {
        use std::fs;
        let preset = root
            .join("node_modules")
            .join("@scope")
            .join("zfb-preset-example");
        fs::create_dir_all(preset.join("dist")).unwrap();
        fs::write(
            preset.join("package.json"),
            r#"{
              "name": "@scope/zfb-preset-example",
              "version": "0.1.0",
              "type": "module",
              "exports": {
                ".": { "import": "./dist/index.mjs", "default": "./dist/index.mjs" },
                "./package.json": "./package.json"
              },
              "main": "./dist/index.mjs"
            }"#,
        )
        .unwrap();
        fs::write(
            preset.join("dist").join("index.mjs"),
            "export default {};\n",
        )
        .unwrap();
        // Relative plugin bundled inside the preset (NOT at the project root).
        fs::write(preset.join("search.js"), "export default {};\n").unwrap();

        // Bare dep nested inside the preset's own node_modules (NOT hoisted to
        // the project root).
        let nested = preset.join("node_modules").join("zfb-plugin-nested");
        fs::create_dir_all(nested.join("dist")).unwrap();
        fs::write(
            nested.join("package.json"),
            r#"{
              "name": "zfb-plugin-nested",
              "version": "0.1.0",
              "type": "module",
              "exports": { ".": { "import": "./dist/index.mjs", "default": "./dist/index.mjs" } },
              "main": "./dist/index.mjs"
            }"#,
        )
        .unwrap();
        fs::write(
            nested.join("dist").join("index.mjs"),
            "export default {};\n",
        )
        .unwrap();
    }

    /// Step 4 (relative-path case): a preset-contributed plugin whose name is a
    /// relative path (`./search.js`) and which carries a `source_package`
    /// marker resolves against the PRESET dir — the file lives only inside the
    /// preset package, NOT at the project root.
    #[test]
    fn marker_anchors_relative_preset_plugin_at_preset_dir() {
        let tmp = TempDir::new().unwrap();
        build_inline_preset_node_modules(tmp.path());

        let mut cfg = Config {
            plugins: vec![PluginConfig {
                name: "./search.js".into(),
                source_package: Some("@scope/zfb-preset-example".into()),
                ..Default::default()
            }],
            ..Config::default()
        };

        resolve_unresolved_plugin_modules(&mut cfg, tmp.path())
            .expect("marked relative preset plugin must resolve from the preset dir");

        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module populated");
        assert!(
            resolved.starts_with("file://"),
            "expected file:// URL, got {resolved}"
        );
        assert!(
            resolved.ends_with("/search.js"),
            "must resolve to search.js, got {resolved}"
        );
        assert!(
            resolved.contains("zfb-preset-example"),
            "must resolve INSIDE the preset dir, got {resolved}"
        );
    }

    /// Step 4 (bare-dep case): a preset-contributed bare plugin
    /// (`zfb-plugin-nested`) installed only inside the preset's own
    /// `node_modules` (not hoisted to the project root) resolves from the
    /// preset dir because of the `source_package` marker.
    #[test]
    fn marker_anchors_bare_preset_plugin_at_preset_node_modules() {
        let tmp = TempDir::new().unwrap();
        build_inline_preset_node_modules(tmp.path());

        let mut cfg = Config {
            plugins: vec![PluginConfig {
                name: "zfb-plugin-nested".into(),
                source_package: Some("@scope/zfb-preset-example".into()),
                ..Default::default()
            }],
            ..Config::default()
        };

        resolve_unresolved_plugin_modules(&mut cfg, tmp.path())
            .expect("marked bare preset plugin must resolve from the preset's node_modules");

        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module populated");
        assert!(
            resolved.contains("zfb-plugin-nested"),
            "must resolve to the nested plugin, got {resolved}"
        );
        assert!(
            resolved.contains("zfb-preset-example"),
            "nested plugin lives under the preset dir, got {resolved}"
        );
    }

    /// Step 4 (graceful degradation): when `source_package` names a package
    /// that cannot be resolved from the project root, resolution degrades to
    /// today's project-root path rather than hard-failing on the package-dir
    /// step. Here the plugin file itself IS present at the project root, so the
    /// fallback succeeds.
    #[test]
    fn marker_degrades_to_project_root_when_package_dir_unresolvable() {
        let tmp = TempDir::new().unwrap();
        // Plugin file present at project root; the named source package is NOT
        // installed (no node_modules entry), so resolve_package_dir errors.
        std::fs::write(tmp.path().join("local.mjs"), "export default {};\n").unwrap();

        let mut cfg = Config {
            plugins: vec![PluginConfig {
                name: "./local.mjs".into(),
                source_package: Some("@scope/not-installed-preset".into()),
                ..Default::default()
            }],
            ..Config::default()
        };

        resolve_unresolved_plugin_modules(&mut cfg, tmp.path())
            .expect("must degrade to project-root resolution when preset dir is unresolvable");

        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module populated via project-root fallback");
        assert!(
            resolved.ends_with("/local.mjs"),
            "must resolve the project-root file, got {resolved}"
        );
    }

    /// Step 4 (degradation error path): when the preset package dir is
    /// unresolvable AND project-root resolution also fails, the surfaced error
    /// names the preset package and states the preset dir could not be
    /// resolved.
    #[test]
    fn marker_degradation_error_names_preset_and_dir_failure() {
        let tmp = TempDir::new().unwrap();
        // No plugin file, no installed package — both the package-dir step and
        // the project-root fallback fail.
        let mut cfg = Config {
            plugins: vec![PluginConfig {
                name: "ghost-plugin".into(),
                source_package: Some("@scope/missing-preset".into()),
                ..Default::default()
            }],
            ..Config::default()
        };

        let err = resolve_unresolved_plugin_modules(&mut cfg, tmp.path())
            .expect_err("both package-dir and project-root resolution fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("@scope/missing-preset"),
            "error must name the preset package, got:\n{msg}"
        );
        assert!(
            msg.contains("package dir could not be resolved"),
            "error must state the preset dir could not be resolved, got:\n{msg}"
        );
    }

    /// TS-path preset scalar field fills in when the main config leaves it default.
    ///
    /// Smoke-test that non-plugin preset fields also flow through correctly on
    /// the TS path (regression guard for Bug 1 ordering — the merge used to run
    /// before the zip-assignment, corrupting the plugin count check).
    #[tokio::test]
    async fn ts_path_preset_scalar_fills_in() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "adapter": "@takazudo/zfb-adapter-cloudflare" }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("preset scalar on TS path should be applied");
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("@takazudo/zfb-adapter-cloudflare"),
            "preset adapter must fill in when main config leaves it absent"
        );
    }

    // --- Multi-preset declared-order (#1191 review, codex P2) ------------------

    /// JSON path: with `presets: [a, b]` each contributing a plugin (and a
    /// user plugin too), the merged plugin order must be the DECLARED order
    /// `a, b, <user plugins>` — NOT the reversed `b, a, user`.
    ///
    /// Regression guard for codex P2 (now via the Value-layer two-phase
    /// fold): the four top-level additive arrays concatenate
    /// `[first preset…, second preset…, user…]`, preserving declared order.
    #[tokio::test]
    async fn json_path_multi_preset_preserves_declared_order() {
        let tmp = TempDir::new().unwrap();
        for name in ["a-plugin.mjs", "b-plugin.mjs", "user-plugin.mjs"] {
            tokio::fs::write(tmp.path().join(name), "export default {};\n")
                .await
                .unwrap();
        }
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "presets": [
                    { "plugins": [{ "name": "./a-plugin.mjs" }] },
                    { "plugins": [{ "name": "./b-plugin.mjs" }] }
                ],
                "plugins": [{ "name": "./user-plugin.mjs" }]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("multi-preset JSON config should load");
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["./a-plugin.mjs", "./b-plugin.mjs", "./user-plugin.mjs"],
            "presets[a, b] must yield declared plugin order a, b, user (codex P2); got {names:?}"
        );
    }

    /// TS path: same declared-order invariant as the JSON test. The TS load
    /// loop shares the prepend-merge pattern, so it carries the same reverse
    /// fold. Plugins are referenced as `.mjs` files so Rust-side resolution
    /// succeeds for the preset-contributed entries.
    #[tokio::test]
    async fn ts_path_multi_preset_preserves_declared_order() {
        let tmp = TempDir::new().unwrap();
        for name in ["a-plugin.mjs", "b-plugin.mjs"] {
            tokio::fs::write(tmp.path().join(name), "export default {};\n")
                .await
                .unwrap();
        }
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "plugins": [{ "name": "./a-plugin.mjs" }] },
                            { "plugins": [{ "name": "./b-plugin.mjs" }] }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("multi-preset TS config should load");
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["./a-plugin.mjs", "./b-plugin.mjs"],
            "presets[a, b] must yield declared plugin order a, b on the TS path (codex P2); got {names:?}"
        );
    }

    // --- Preset nested-block deep merge (#1191 review [11]) --------------------

    /// A preset contributing `markdown.features.githubAlerts` must SURVIVE the
    /// user setting an unrelated `markdown` sibling (`gfm`). The old
    /// whole-block `fill_default!` dropped the entire preset markdown block the
    /// moment the user touched any sibling — this is the documented intended
    /// use (a zudo-doc-style markdown preset) and the core of finding [11].
    #[test]
    fn preset_markdown_block_deep_merges_with_user_sibling() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "markdown": { "features": { "githubAlerts": true } }
            })],
            serde_json::json!({
                "markdown": { "gfm": true }
            }),
        );

        let md = cfg.markdown.expect("markdown block must be present");
        assert_eq!(
            md.gfm,
            Some(GfmFlag::All(true)),
            "user's markdown.gfm must survive"
        );
        let features = md
            .features
            .expect("preset's markdown.features must survive the user's sibling");
        assert_eq!(
            features.github_alerts,
            Some(FeatureToggle::Bool(true)),
            "preset's markdown.features.githubAlerts must be merged in, not dropped"
        );
    }

    /// User-set inner fields win over a preset's value for the SAME inner
    /// field (per-field user authority), while non-overlapping preset inner
    /// fields still fill in.
    #[test]
    fn preset_markdown_inner_field_user_wins_but_others_fill() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "markdown": {
                    "gfm": false,        // collides with user
                    "hardBreaks": true   // user left unset
                }
            })],
            serde_json::json!({
                "markdown": { "gfm": true }
            }),
        );

        let md = cfg.markdown.unwrap();
        assert_eq!(
            md.gfm,
            Some(GfmFlag::All(true)),
            "user's gfm must win the per-field collision"
        );
        assert_eq!(
            md.hard_breaks,
            Some(true),
            "preset's non-overlapping hardBreaks must fill in"
        );
    }

    /// A preset `bundle` block must deep-merge with a user `bundle` sibling
    /// (the same trap as markdown, applied to a different block).
    #[test]
    fn preset_bundle_block_deep_merges_with_user_sibling() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "bundle": { "external": ["msw"] }
            })],
            serde_json::json!({
                "bundle": { "exclude": ["**/*.stories.tsx"] }
            }),
        );

        let bundle = cfg.bundle.unwrap();
        assert_eq!(
            bundle.exclude,
            Some(vec!["**/*.stories.tsx".into()]),
            "user's bundle.exclude must survive"
        );
        assert_eq!(
            bundle.external,
            Some(vec!["msw".into()]),
            "preset's bundle.external must be merged in, not dropped"
        );
    }

    // --- #1202: arbitrary-depth sibling survival ------------------------------

    /// #1202 (3rd level): user sets `markdown.features.{fieldA}` and a preset
    /// sets a DIFFERENT sibling `markdown.features.{fieldB}` → both survive.
    /// `markdown.features.codeTabs` (user) vs `markdown.features.ruby`
    /// (preset) are real sibling fields of `MarkdownFeaturesConfig` — the
    /// issue's literal example used `headingIds.{fieldA/fieldB}`, but
    /// `HeadingIdsConfig` has a single field (`strategy`) and
    /// `deny_unknown_fields`, so two distinct siblings live one level up under
    /// `features`.
    #[test]
    fn preset_markdown_features_third_level_siblings_both_survive() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "markdown": { "features": { "ruby": true } }
            })],
            serde_json::json!({
                "markdown": { "features": { "codeTabs": true } }
            }),
        );
        let features = cfg
            .markdown
            .expect("markdown present")
            .features
            .expect("features present");
        assert_eq!(
            features.code_tabs,
            Some(FeatureToggle::Bool(true)),
            "user's features.codeTabs must survive"
        );
        assert_eq!(
            features.ruby,
            Some(FeatureToggle::Bool(true)),
            "preset's sibling features.ruby must survive"
        );
    }

    /// #1202 (4th level): user sets
    /// `markdown.features.codeEnrichment.diffMarkers` and a preset sets the
    /// DIFFERENT siblings `markdown.features.codeEnrichment.lineHighlight`
    /// and `wordHighlight` → all survive. This is a genuine 4th-level object collision (markdown →
    /// features → codeEnrichment → {diffMarkers, lineHighlight,
    /// wordHighlight}).
    #[test]
    fn preset_markdown_features_fourth_level_siblings_both_survive() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "markdown": { "features": { "codeEnrichment": {
                    "lineHighlight": false,
                    "wordHighlight": true
                } } }
            })],
            serde_json::json!({
                "markdown": { "features": { "codeEnrichment": { "diffMarkers": false } } }
            }),
        );
        let enrichment = cfg
            .markdown
            .expect("markdown present")
            .features
            .expect("features present")
            .code_enrichment
            .expect("codeEnrichment present");
        assert_eq!(
            enrichment.diff_markers,
            Some(false),
            "user's codeEnrichment.diffMarkers must survive (presence, not equals-default)"
        );
        assert_eq!(
            enrichment.line_highlight,
            Some(false),
            "preset's sibling codeEnrichment.lineHighlight must survive"
        );
        assert_eq!(
            enrichment.word_highlight,
            Some(true),
            "preset's sibling codeEnrichment.wordHighlight must survive"
        );
    }

    // --- #1199: presence vs equals-default ------------------------------------

    /// #1199: the user EXPLICITLY sets a scalar to a value equal to its type
    /// default (`copyPublicWithBase` defaults to `true`; user sets `true`)
    /// while a preset sets the opposite (`false`). The user's explicit value
    /// must win — the merge keys on key PRESENCE, not value-equals-default.
    /// The old typed fold gave this to the preset.
    #[test]
    fn preset_user_explicit_default_value_beats_preset() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "copyPublicWithBase": false })],
            serde_json::json!({ "copyPublicWithBase": true }),
        );
        assert!(
            cfg.copy_public_with_base,
            "user's explicit `true` (== type default) must beat the preset's `false`"
        );
    }

    #[test]
    fn preset_minify_html_true_fills_when_user_omits() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "minifyHtml": true })],
            serde_json::json!({}),
        );
        assert!(
            cfg.minify_html,
            "preset minifyHtml:true must apply when the user omits the field"
        );
    }

    #[test]
    fn preset_user_minify_html_false_beats_preset_true() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "minifyHtml": true })],
            serde_json::json!({ "minifyHtml": false }),
        );
        assert!(
            !cfg.minify_html,
            "user's explicit minifyHtml:false must beat preset true"
        );
    }

    /// Control for #1199: when the user OMITS the key, the preset fills it.
    #[test]
    fn preset_fills_when_user_omits_default_value_key() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "copyPublicWithBase": false })],
            serde_json::json!({}),
        );
        assert!(
            !cfg.copy_public_with_base,
            "with the key omitted, the preset's `false` fills in"
        );
    }

    // --- null blocks the preset -----------------------------------------------

    /// An explicit `null` in the user config blocks the preset value (the user
    /// opted out). `adapter: null` → the preset's adapter does NOT fill in.
    #[test]
    fn preset_user_null_blocks_preset_value() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({ "adapter": "preset-adapter" })],
            serde_json::json!({ "adapter": null }),
        );
        assert_eq!(
            cfg.adapter, None,
            "explicit `adapter: null` must block the preset's adapter"
        );
    }

    /// `markdown: null` blocks a whole preset block.
    #[test]
    fn preset_user_null_blocks_preset_block() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "markdown": { "features": { "githubAlerts": true } }
            })],
            serde_json::json!({ "markdown": null }),
        );
        assert_eq!(
            cfg.markdown, None,
            "explicit `markdown: null` must block the preset's markdown block"
        );
    }

    // --- map-like recursion ---------------------------------------------------

    /// Preset and user contribute DIFFERENT sibling entries under a map-like
    /// object (`markdown.features.directives` is a `HashMap<String,
    /// DirectiveSpec>`) → both entries survive.
    #[test]
    fn preset_map_like_directives_siblings_both_survive() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "markdown": { "features": { "directives": { "tip": "Tip" } } }
            })],
            serde_json::json!({
                "markdown": { "features": { "directives": { "note": "Note" } } }
            }),
        );
        let directives = cfg
            .markdown
            .expect("markdown present")
            .features
            .expect("features present")
            .directives
            .expect("directives present");
        assert!(
            directives.contains_key("note"),
            "user's directive entry `note` must survive"
        );
        assert!(
            directives.contains_key("tip"),
            "preset's directive entry `tip` must survive"
        );
        assert_eq!(directives.len(), 2, "both map entries present");
    }

    // --- additive-array order across presets + user ---------------------------

    /// `presets: [a, b]` each contributing `plugins` plus a user plugin →
    /// merged order is `[a, b, user]`.
    #[test]
    fn preset_additive_plugins_order_a_b_user() {
        let cfg = merge_presets_to_config(
            vec![
                serde_json::json!({ "plugins": [{ "name": "a", "options": {} }] }),
                serde_json::json!({ "plugins": [{ "name": "b", "options": {} }] }),
            ],
            serde_json::json!({ "plugins": [{ "name": "user", "options": {} }] }),
        );
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "user"]);
    }

    /// Same additive `[a, b, user]` order for `collections`.
    #[test]
    fn preset_additive_collections_order_a_b_user() {
        let cfg = merge_presets_to_config(
            vec![
                serde_json::json!({ "collections": [{ "name": "a", "path": "content/a" }] }),
                serde_json::json!({ "collections": [{ "name": "b", "path": "content/b" }] }),
            ],
            serde_json::json!({ "collections": [{ "name": "user", "path": "content/user" }] }),
        );
        let names: Vec<&str> = cfg.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "user"]);
    }

    /// Same additive `[a, b, user]` order for the string-valued
    /// `allowedHosts` (covers an additive array whose items are scalars).
    #[test]
    fn preset_additive_allowed_hosts_order_a_b_user() {
        let cfg = merge_presets_to_config(
            vec![
                serde_json::json!({ "allowedHosts": ["a.example.com"] }),
                serde_json::json!({ "allowedHosts": ["b.example.com"] }),
            ],
            serde_json::json!({ "allowedHosts": ["user.example.com"] }),
        );
        assert_eq!(
            cfg.allowed_hosts,
            vec![
                "a.example.com".to_string(),
                "b.example.com".to_string(),
                "user.example.com".to_string()
            ]
        );
    }

    /// Same additive `[a, b, user]` order for `extraWatchPaths`.
    #[test]
    fn preset_additive_extra_watch_paths_order_a_b_user() {
        let cfg = merge_presets_to_config(
            vec![
                serde_json::json!({ "extraWatchPaths": ["/abs/a"] }),
                serde_json::json!({ "extraWatchPaths": ["/abs/b"] }),
            ],
            serde_json::json!({ "extraWatchPaths": ["/abs/user"] }),
        );
        let paths: Vec<&str> = cfg
            .extra_watch_paths
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["/abs/a", "/abs/b", "/abs/user"]);
    }

    /// A `presets` key nested INSIDE a preset object is stripped — never
    /// recursively expanded (#1196). The inner preset's scalar must NOT leak.
    #[test]
    fn preset_nested_presets_key_is_stripped() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "adapter": "outer-adapter",
                "presets": [{ "adapter": "should-not-appear", "strip_md_ext": true }]
            })],
            serde_json::json!({}),
        );
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("outer-adapter"),
            "the outer preset's adapter applies"
        );
        assert!(
            !cfg.strip_md_ext,
            "a nested `presets` key must be stripped, not expanded"
        );
    }

    /// Nested arrays are NOT additive: a user `bundle.exclude` wins WHOLE over
    /// a preset `bundle.exclude` (no concat) — the additive rule is top-level
    /// only.
    #[test]
    fn preset_nested_array_user_wins_not_additive() {
        let cfg = merge_presets_to_config(
            vec![serde_json::json!({
                "bundle": { "exclude": ["preset-only.tsx"] }
            })],
            serde_json::json!({
                "bundle": { "exclude": ["user-only.tsx"] }
            }),
        );
        assert_eq!(
            cfg.bundle.unwrap().exclude,
            Some(vec!["user-only.tsx".to_string()]),
            "user's bundle.exclude wins whole; nested arrays are not additive"
        );
    }

    /// A non-array `presets` (e.g. the malformed TS shape `presets: somePreset()`
    /// instead of `[somePreset()]`) must be REJECTED, not silently dropped — the
    /// pre-#1196 typed `presets: Vec<Value>` deserialization rejected it, and the
    /// Value-layer merge must preserve that (codex review of #1199/#1202).
    #[test]
    fn take_presets_rejects_non_array() {
        // object value → error
        let mut obj = serde_json::json!({ "presets": { "adapter": "x" } });
        assert!(
            take_presets(&mut obj).is_err(),
            "a non-array `presets` object must be rejected"
        );
        // string value → error
        let mut s = serde_json::json!({ "presets": "nope" });
        assert!(
            take_presets(&mut s).is_err(),
            "a non-array `presets` string must be rejected"
        );
        // absent → Ok(None)
        let mut absent = serde_json::json!({ "adapter": "x" });
        assert_eq!(
            take_presets(&mut absent),
            Ok(None),
            "absent presets is fine"
        );
        // empty array → Ok(None) and the key is stripped
        let mut empty = serde_json::json!({ "presets": [] });
        assert_eq!(
            take_presets(&mut empty),
            Ok(None),
            "an empty presets array is no-op"
        );
        assert!(
            empty.get("presets").is_none(),
            "the empty presets key is removed in place"
        );
        // non-empty array → Ok(Some(..))
        let mut arr = serde_json::json!({ "presets": [{ "adapter": "p" }] });
        assert_eq!(
            take_presets(&mut arr).map(|o| o.map(|v| v.len())),
            Ok(Some(1)),
            "a non-empty presets array is returned"
        );
    }

    /// First-declared preset wins a shared scalar at the Value layer, matching
    /// the additive-array precedence (`a` before `b`).
    #[test]
    fn preset_multi_preset_scalar_first_declared_wins() {
        let cfg = merge_presets_to_config(
            vec![
                serde_json::json!({ "adapter": "adapter-a" }),
                serde_json::json!({ "adapter": "adapter-b" }),
            ],
            serde_json::json!({}),
        );
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("adapter-a"),
            "first-declared preset's adapter wins"
        );
    }

    /// End-to-end JSON path: the deep-merge holds through the real loader.
    #[tokio::test]
    async fn json_path_preset_markdown_block_deep_merges() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "presets": [
                    { "markdown": { "features": { "githubAlerts": true } } }
                ],
                "markdown": { "gfm": true }
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("preset+user markdown JSON config should load");
        let md = cfg.markdown.expect("markdown block present");
        assert_eq!(md.gfm, Some(GfmFlag::All(true)), "user gfm survives");
        let features = md.features.expect("preset features survive");
        assert_eq!(
            features.github_alerts,
            Some(FeatureToggle::Bool(true)),
            "preset features.githubAlerts merged via the real loader"
        );
    }

    // --- Multi-preset scalar precedence (#1191 review [12]) --------------------

    /// JSON path: with `presets: [a, b]` BOTH setting the same scalar
    /// (`adapter`) AND both contributing a plugin, the first-declared preset
    /// (`a`) must win the scalar — consistent with the array precedence
    /// (`a` before `b`). The old reverse fold gave the scalar to `b` (last
    /// declared) while arrays went to `a`, an inconsistency (finding [12]).
    #[tokio::test]
    async fn json_path_multi_preset_scalar_first_declared_wins() {
        let tmp = TempDir::new().unwrap();
        for name in ["a-plugin.mjs", "b-plugin.mjs"] {
            tokio::fs::write(tmp.path().join(name), "export default {};\n")
                .await
                .unwrap();
        }
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "presets": [
                    { "adapter": "adapter-a", "plugins": [{ "name": "./a-plugin.mjs" }] },
                    { "adapter": "adapter-b", "plugins": [{ "name": "./b-plugin.mjs" }] }
                ]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("multi-preset scalar JSON config should load");
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("adapter-a"),
            "first-declared preset's adapter must win, consistent with array order"
        );
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["./a-plugin.mjs", "./b-plugin.mjs"],
            "plugin order must be a-before-b (first declared first)"
        );
    }

    /// TS path: same first-declared-preset-wins invariant for a shared scalar.
    #[tokio::test]
    async fn ts_path_multi_preset_scalar_first_declared_wins() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "adapter": "adapter-a" },
                            { "adapter": "adapter-b" }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("multi-preset scalar TS config should load");
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("adapter-a"),
            "first-declared preset's adapter must win on the TS path too"
        );
    }

    /// The user always beats BOTH presets on a shared scalar (preset
    /// precedence must never clobber the user's value).
    #[tokio::test]
    async fn json_path_user_scalar_beats_all_presets() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "adapter": "adapter-user",
                "presets": [
                    { "adapter": "adapter-a" },
                    { "adapter": "adapter-b" }
                ]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("user-scalar-vs-presets JSON config should load");
        assert_eq!(
            cfg.adapter.as_deref(),
            Some("adapter-user"),
            "the user's explicit adapter must beat every preset"
        );
    }

    // --- T5 preset-provenance end-to-end tests (#1217) -----------------------

    /// T5.1 — CONFIRM (real capture-chain): the `definePreset(sourcePackage, …)`
    /// string literal survives esbuild bundling and V8 evaluation end-to-end.
    ///
    /// This is the authoritative proof that provenance flows all the way from
    /// `zfb.config.ts` (imports a preset that calls `definePreset`) → esbuild
    /// bundle (alias rewrites `@takazudo/zfb/config` to the stub) → V8 eval
    /// (stub stamps `source_package`) → Rust resolver (anchors at preset dir).
    ///
    /// Gated on `embed_v8` (the default feature). CI has V8 compiled in, so
    /// this test runs in `cargo test -p zfb` at the PR gate without `#[ignore]`.
    /// The slim-build path (no V8) uses the node subprocess instead, which has
    /// the same esbuild alias and identical stub — the capture chain is the same;
    /// only the evaluator differs.
    ///
    /// **Slim-path companion:** the slim-build (`--no-default-features`) node
    /// subprocess path is exercised by
    /// `e2e_capture_chain_define_preset_source_package_survives_esbuild_slim`
    /// (below, gated `cfg(not(feature = "embed_v8"))`). The PR gate's `build-no-v8`
    /// job compile-checks it via `cargo check --no-default-features -p zfb --tests`,
    /// and it runs locally under `cargo test --no-default-features -p zfb`.
    /// **Executing** slim tests in CI (not just compiling them) still needs a T3
    /// slim-test lane and remains deferred — so the slim path is compile-covered at
    /// the gate + runnable locally, not yet CI-executed.
    #[cfg(feature = "embed_v8")]
    #[tokio::test]
    async fn e2e_capture_chain_define_preset_source_package_survives_esbuild_v8() {
        use std::fs;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // --- Stage the preset package in <root>/node_modules ---
        //
        // Package: @scope/zfb-preset-e2e
        // It contains:
        //   - package.json   (with exports so resolve_package_dir can find it)
        //   - e2e-plugin.mjs (the relative plugin contributed by the preset)
        //   - dist/index.mjs (the preset's main export, imports from
        //                     @takazudo/zfb/config and calls definePreset)
        //
        // esbuild runs with --alias:@takazudo/zfb/config=<stub>, so the preset's
        // `import { definePreset } from "@takazudo/zfb/config"` is rewritten to
        // the stub at bundle time. The stub stamps each plugin with
        // `source_package: "@scope/zfb-preset-e2e"`.
        let preset_dir = root
            .join("node_modules")
            .join("@scope")
            .join("zfb-preset-e2e");
        fs::create_dir_all(preset_dir.join("dist")).unwrap();

        // Relative plugin file — only present INSIDE the preset dir, NOT at the
        // project root. If `source_package` does NOT survive esbuild→V8, the
        // resolver falls back to project root and fails to find the file (the
        // file is missing from root), which would cause the load to error.
        fs::write(preset_dir.join("e2e-plugin.mjs"), "export default {};\n").unwrap();

        // preset package.json — needs `exports: { "./package.json" }` so
        // `resolve_package_dir` (T1) can resolve the package root via oxc_resolver.
        fs::write(
            preset_dir.join("package.json"),
            r#"{
              "name": "@scope/zfb-preset-e2e",
              "version": "0.1.0",
              "type": "module",
              "exports": {
                ".": { "import": "./dist/index.mjs", "default": "./dist/index.mjs" },
                "./package.json": "./package.json"
              },
              "main": "./dist/index.mjs"
            }"#,
        )
        .unwrap();

        // Preset entry-point: calls definePreset so the plugin gets stamped.
        // esbuild will alias `@takazudo/zfb/config` → stub at bundle time.
        fs::write(
            preset_dir.join("dist").join("index.mjs"),
            r#"import { definePreset } from "@takazudo/zfb/config";
export default definePreset("@scope/zfb-preset-e2e", {
  plugins: [{ name: "./e2e-plugin.mjs" }],
});
"#,
        )
        .unwrap();

        // --- User config: imports the preset and spreads it into presets[] ---
        tokio::fs::write(
            root.join("zfb.config.ts"),
            r#"import preset from "@scope/zfb-preset-e2e";
export default {
  presets: [preset],
};
"#,
        )
        .await
        .unwrap();

        // Load via the REAL esbuild + V8 path (no test_default_export_json override).
        let cfg = load_from_dir(root)
            .await
            .expect("e2e preset-provenance config must load successfully");

        // The preset contributes one plugin. It should be present in the merged config.
        assert_eq!(
            cfg.plugins.len(),
            1,
            "preset-contributed plugin must appear in merged config"
        );
        assert_eq!(
            cfg.plugins[0].name, "./e2e-plugin.mjs",
            "plugin name must match the preset's declaration"
        );

        // The plugin must be resolved — and it must resolve to the PRESET dir,
        // not the project root (the file only exists inside the preset package).
        let resolved = cfg.plugins[0].resolved_module.as_deref().expect(
            "plugin must have resolved_module populated (source_package survived bundling)",
        );

        assert!(
            resolved.starts_with("file://"),
            "resolved_module must be a file:// URL, got {resolved:?}"
        );
        assert!(
            resolved.contains("zfb-preset-e2e"),
            "resolved_module must point into the preset package dir, got {resolved:?}"
        );
        assert!(
            resolved.ends_with("/e2e-plugin.mjs"),
            "resolved_module must name the plugin file, got {resolved:?}"
        );

        // Canonicalize to handle macOS /tmp → /private/tmp redirect.
        let resolved_path = url::Url::parse(resolved)
            .expect("valid file:// URL")
            .to_file_path()
            .expect("file URL round-trips to path");
        let expected_path = preset_dir
            .join("e2e-plugin.mjs")
            .canonicalize()
            .expect("preset plugin file exists and is canonicalisable");
        assert_eq!(
            resolved_path
                .canonicalize()
                .unwrap_or(resolved_path.clone()),
            expected_path,
            "resolved_module must point at the preset's e2e-plugin.mjs, not the project root"
        );
    }

    /// T5.1-slim — CONFIRM (real capture-chain, slim build): the slim companion to
    /// `e2e_capture_chain_define_preset_source_package_survives_esbuild_v8`. It proves
    /// the `definePreset(sourcePackage, …)` provenance survives the **node subprocess**
    /// evaluator (the `--no-default-features` slim path, `config-loader.mjs`) end-to-end,
    /// not just the in-process V8 evaluator. The fixture, esbuild alias, `definePreset`
    /// stub, and Rust-side resolution are identical to the V8 test — only the JS evaluator
    /// differs — so this is a line-for-line mirror that swaps the evaluator.
    ///
    /// **CI coverage:** the `build-no-v8` gate compile-checks this test via
    /// `cargo check --no-default-features -p zfb --tests`; it is **not executed** in CI
    /// until a T3 slim-test lane exists. Run it locally with
    /// `cargo test --no-default-features -p zfb`.
    ///
    /// Self-skips (returns) when its toolchain is genuinely absent: esbuild not found by
    /// `zfb_test_utils::locate_esbuild()` (the canonical probe — which *panics* rather than
    /// skip if the workspace slot binary is present but unresolved, per #1007, so a silent
    /// no-coverage skip cannot happen), or no runnable `node` on PATH (this is the first
    /// *positive* slim test that actually spawns node, so it guards on node availability
    /// rather than hard-fail on a node-less host).
    #[cfg(not(feature = "embed_v8"))]
    #[tokio::test]
    async fn e2e_capture_chain_define_preset_source_package_survives_esbuild_slim() {
        use std::fs;

        // Skip guard 1 — esbuild: locate the real binary via the canonical test-utils
        // probe. Unlike a CWD-relative `DEFAULT_ESBUILD_SLOT` existence check, this walks
        // absolute candidate workspace roots, so it actually finds the workspace slot
        // under the documented `cargo test --no-default-features -p zfb` run (cwd is the
        // crate dir, not the workspace root) instead of silently skipping and providing
        // zero coverage. It also trips a #1007 panic if the slot binary exists but lookup
        // fails. The located path is threaded explicitly through `esbuild_binary` below so
        // the load is deterministic (no reliance on the loader's CWD-relative slot leg).
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            return;
        };

        // Skip guard 2 — node availability: the slim path spawns `node` to evaluate the
        // bundled config. Probe it (status check, null stdio — catches "spawned but
        // unusable", avoids captured noise) and skip rather than fail when node is not on
        // PATH. No race: if node vanishes between probe and load, the test just fails,
        // which is acceptable.
        fn host_node_available() -> bool {
            std::process::Command::new("node")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        if !host_node_available() {
            return;
        }

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // --- Stage the preset package in <root>/node_modules ---
        //
        // Package: @scope/zfb-preset-e2e
        // It contains:
        //   - package.json   (with exports so resolve_package_dir can find it)
        //   - e2e-plugin.mjs (the relative plugin contributed by the preset)
        //   - dist/index.mjs (the preset's main export, imports from
        //                     @takazudo/zfb/config and calls definePreset)
        //
        // esbuild runs with --alias:@takazudo/zfb/config=<stub>, so the preset's
        // `import { definePreset } from "@takazudo/zfb/config"` is rewritten to
        // the stub at bundle time. The stub stamps each plugin with
        // `source_package: "@scope/zfb-preset-e2e"`.
        let preset_dir = root
            .join("node_modules")
            .join("@scope")
            .join("zfb-preset-e2e");
        fs::create_dir_all(preset_dir.join("dist")).unwrap();

        // Relative plugin file — only present INSIDE the preset dir, NOT at the
        // project root. If `source_package` does NOT survive esbuild→node, the
        // resolver falls back to project root and fails to find the file (the
        // file is missing from root), which would cause the load to error.
        fs::write(preset_dir.join("e2e-plugin.mjs"), "export default {};\n").unwrap();

        // preset package.json — needs `exports: { "./package.json" }` so
        // `resolve_package_dir` (T1) can resolve the package root via oxc_resolver.
        fs::write(
            preset_dir.join("package.json"),
            r#"{
              "name": "@scope/zfb-preset-e2e",
              "version": "0.1.0",
              "type": "module",
              "exports": {
                ".": { "import": "./dist/index.mjs", "default": "./dist/index.mjs" },
                "./package.json": "./package.json"
              },
              "main": "./dist/index.mjs"
            }"#,
        )
        .unwrap();

        // Preset entry-point: calls definePreset so the plugin gets stamped.
        // esbuild will alias `@takazudo/zfb/config` → stub at bundle time.
        fs::write(
            preset_dir.join("dist").join("index.mjs"),
            r#"import { definePreset } from "@takazudo/zfb/config";
export default definePreset("@scope/zfb-preset-e2e", {
  plugins: [{ name: "./e2e-plugin.mjs" }],
});
"#,
        )
        .unwrap();

        // --- User config: imports the preset and spreads it into presets[] ---
        tokio::fs::write(
            root.join("zfb.config.ts"),
            r#"import preset from "@scope/zfb-preset-e2e";
export default {
  presets: [preset],
};
"#,
        )
        .await
        .unwrap();

        // Load via the REAL esbuild + node-subprocess path (slim build). Pass the located
        // esbuild explicitly (deterministic — no reliance on the loader's CWD-relative slot
        // resolution), but leave `node_binary` unset so the genuine `node` subprocess
        // evaluator runs, and `test_default_export_json` unset so esbuild + node really
        // execute (not the envelope bypass).
        let opts = LoadOptions {
            esbuild_binary: Some(esbuild),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(root, &opts)
            .await
            .expect("e2e preset-provenance config (slim) must load successfully");

        // The preset contributes one plugin. It should be present in the merged config.
        assert_eq!(
            cfg.plugins.len(),
            1,
            "preset-contributed plugin must appear in merged config"
        );
        assert_eq!(
            cfg.plugins[0].name, "./e2e-plugin.mjs",
            "plugin name must match the preset's declaration"
        );

        // The plugin must be resolved — and it must resolve to the PRESET dir,
        // not the project root (the file only exists inside the preset package).
        let resolved = cfg.plugins[0].resolved_module.as_deref().expect(
            "plugin must have resolved_module populated (source_package survived bundling)",
        );

        assert!(
            resolved.starts_with("file://"),
            "resolved_module must be a file:// URL, got {resolved:?}"
        );
        assert!(
            resolved.contains("zfb-preset-e2e"),
            "resolved_module must point into the preset package dir, got {resolved:?}"
        );
        assert!(
            resolved.ends_with("/e2e-plugin.mjs"),
            "resolved_module must name the plugin file, got {resolved:?}"
        );

        // Canonicalize to handle macOS /tmp → /private/tmp redirect.
        let resolved_path = url::Url::parse(resolved)
            .expect("valid file:// URL")
            .to_file_path()
            .expect("file URL round-trips to path");
        let expected_path = preset_dir
            .join("e2e-plugin.mjs")
            .canonicalize()
            .expect("preset plugin file exists and is canonicalisable");
        assert_eq!(
            resolved_path
                .canonicalize()
                .unwrap_or(resolved_path.clone()),
            expected_path,
            "resolved_module must point at the preset's e2e-plugin.mjs, not the project root"
        );
    }

    /// T5.2 — Rust-threading via envelope: a `source_package`-stamped preset
    /// plugin injected through `test_default_export_json` (bypasses esbuild+V8,
    /// tests only the Rust resolver threading) resolves to the preset dir.
    ///
    /// Mirrors `ts_path_preset_plugin_is_resolved` but additionally verifies
    /// that the `source_package` field routes resolution to the preset package
    /// dir rather than the project root. The plugin file exists ONLY inside the
    /// preset package; without the provenance routing it would not be found.
    #[tokio::test]
    async fn ts_path_source_package_stamped_plugin_resolves_to_preset_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Stage the preset node_modules tree (same helper used by T4 tests).
        build_inline_preset_node_modules(root);

        // Write a minimal zfb.config.ts (required for the TS load path).
        tokio::fs::write(root.join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();

        // Envelope: a preset contributes a plugin with `source_package` stamped.
        // The plugin file is ONLY inside the preset dir — not at the project root.
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            {
                                "plugins": [{
                                    "name": "./search.js",
                                    "source_package": "@scope/zfb-preset-example"
                                }]
                            }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };

        let cfg = load_from_dir_with_options(root, &opts)
            .await
            .expect("source_package-stamped preset plugin must resolve without error");

        assert_eq!(cfg.plugins.len(), 1, "plugin must appear in merged config");
        assert_eq!(cfg.plugins[0].name, "./search.js");

        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module must be populated");
        assert!(
            resolved.starts_with("file://"),
            "resolved_module must be a file:// URL, got {resolved:?}"
        );
        assert!(
            resolved.contains("zfb-preset-example"),
            "must resolve INSIDE the preset package dir, got {resolved:?}"
        );
        assert!(
            resolved.ends_with("/search.js"),
            "must resolve to search.js, got {resolved:?}"
        );

        // Canonicalize to handle macOS /tmp → /private/tmp redirect.
        let preset_dir = root
            .join("node_modules")
            .join("@scope")
            .join("zfb-preset-example");
        let resolved_path = url::Url::parse(resolved)
            .expect("valid file:// URL")
            .to_file_path()
            .expect("file URL round-trips to path");
        let expected_path = preset_dir
            .join("search.js")
            .canonicalize()
            .expect("preset search.js exists");
        assert_eq!(
            resolved_path
                .canonicalize()
                .unwrap_or(resolved_path.clone()),
            expected_path,
            "resolved_module must point at the preset's search.js, not project root"
        );
    }

    /// T5.3 — Precedence: when the same plugin file name exists at BOTH the
    /// project root AND inside the preset dir, the `source_package` marker
    /// routes resolution to the PRESET copy (preset-dir-first).
    ///
    /// Without the provenance marker the project-root copy would win (it is
    /// found first by the path resolver). With the marker the preset dir is
    /// tried first, so the preset's copy wins.
    #[test]
    fn preset_dir_wins_over_project_root_when_same_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Stage the preset tree (includes search.js at preset dir).
        build_inline_preset_node_modules(root);

        // Also place a file with the SAME name at the project root. Without the
        // provenance-first routing this project-root file would be resolved.
        std::fs::write(root.join("search.js"), "// project-root decoy\n").unwrap();

        let mut cfg = Config {
            plugins: vec![PluginConfig {
                name: "./search.js".into(),
                source_package: Some("@scope/zfb-preset-example".into()),
                ..Default::default()
            }],
            ..Config::default()
        };

        resolve_unresolved_plugin_modules(&mut cfg, root)
            .expect("preset-dir-first resolution must succeed");

        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module populated");

        assert!(
            resolved.contains("zfb-preset-example"),
            "must resolve to the PRESET copy of search.js (not project root), got {resolved:?}"
        );

        // Verify via canonical paths: the resolved URL must point into the
        // preset dir, not the project root (handles /tmp → /private/tmp on macOS).
        let resolved_path = url::Url::parse(resolved)
            .expect("valid file:// URL")
            .to_file_path()
            .expect("file URL to path");
        let preset_copy = root
            .join("node_modules")
            .join("@scope")
            .join("zfb-preset-example")
            .join("search.js")
            .canonicalize()
            .expect("preset search.js must exist");
        let project_root_copy = root
            .join("search.js")
            .canonicalize()
            .expect("project-root search.js must exist");

        assert_eq!(
            resolved_path
                .canonicalize()
                .unwrap_or(resolved_path.clone()),
            preset_copy,
            "preset copy must win; got {resolved:?}"
        );
        assert_ne!(
            resolved_path.canonicalize().unwrap_or(resolved_path),
            project_root_copy,
            "project-root copy must NOT win when source_package is set"
        );
    }

    /// T5.4 — Graceful degradation (backward-compat): a preset plugin with NO
    /// `source_package` marker (e.g. a preset that was not authored with
    /// `definePreset`, or a JSON-inline preset object) still resolves against
    /// the project root — the pre-provenance behavior is preserved.
    #[tokio::test]
    async fn ts_path_no_source_package_preset_plugin_resolves_at_project_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Plugin file at the PROJECT ROOT (no preset node_modules layout).
        tokio::fs::write(root.join("legacy-plugin.mjs"), "export default {};\n")
            .await
            .unwrap();
        tokio::fs::write(root.join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();

        // Envelope: preset plugin has no source_package → must fall back to
        // project-root resolution (the pre-provenance behavior).
        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "plugins": [{ "name": "./legacy-plugin.mjs" }] }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };

        let cfg = load_from_dir_with_options(root, &opts)
            .await
            .expect("no-source_package preset plugin must resolve at project root");

        assert_eq!(cfg.plugins.len(), 1);
        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module populated");
        assert!(
            resolved.starts_with("file://"),
            "must be a file:// URL, got {resolved:?}"
        );

        // Verify the resolved path is the project-root file.
        let resolved_path = url::Url::parse(resolved)
            .expect("valid file:// URL")
            .to_file_path()
            .expect("file URL round-trips");
        let expected = root
            .join("legacy-plugin.mjs")
            .canonicalize()
            .expect("project-root plugin must exist");
        assert_eq!(
            resolved_path.canonicalize().unwrap_or(resolved_path),
            expected,
            "no-source_package preset plugin must resolve at the project root"
        );
    }

    /// T5.4b — Graceful degradation (error path): a preset plugin with NO
    /// `source_package` and an unresolvable specifier surfaces the clearer
    /// "contributed by a preset" error message (T2 wording).
    #[tokio::test]
    async fn ts_path_no_source_package_unresolvable_preset_plugin_yields_preset_error() {
        let tmp = TempDir::new().unwrap();

        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();

        let opts = LoadOptions {
            test_default_export_json: Some(
                serde_json::json!({
                    "config": {
                        "presets": [
                            { "plugins": [{ "name": "no-such-pkg-for-t5" }] }
                        ]
                    },
                    "plugins": []
                })
                .to_string(),
            ),
            ..LoadOptions::default()
        };

        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("unresolvable no-marker preset plugin must fail");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("contributed by a preset"),
            "error must flag preset origin (T2 wording); got:\n{msg}"
        );
        assert!(
            msg.contains("no-such-pkg-for-t5"),
            "error must name the specifier; got:\n{msg}"
        );
    }

    /// T5.5 — JSON-path coverage: a JSON config's preset (no `definePreset` call,
    /// so no `source_package` marker) still anchors plugin resolution at the
    /// project root (unchanged behavior for JSON configs).
    #[tokio::test]
    async fn json_path_no_source_package_preset_plugin_resolves_at_project_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Plugin lives at the project root only — no preset node_modules.
        tokio::fs::write(root.join("json-preset-plugin.mjs"), "export default {};\n")
            .await
            .unwrap();

        tokio::fs::write(
            root.join("zfb.config.json"),
            r#"{
                "presets": [
                    { "plugins": [{ "name": "./json-preset-plugin.mjs" }] }
                ]
            }"#,
        )
        .await
        .unwrap();

        let cfg = load_from_dir(root)
            .await
            .expect("JSON preset with no source_package must load and resolve at project root");

        assert_eq!(cfg.plugins.len(), 1, "preset plugin must be present");
        let resolved = cfg.plugins[0]
            .resolved_module
            .as_deref()
            .expect("resolved_module populated");
        assert!(
            resolved.starts_with("file://"),
            "must be a file:// URL, got {resolved:?}"
        );

        // The resolved path must point at the project-root copy.
        let resolved_path = url::Url::parse(resolved)
            .expect("valid file:// URL")
            .to_file_path()
            .expect("file URL round-trips");
        let expected = root
            .join("json-preset-plugin.mjs")
            .canonicalize()
            .expect("project-root plugin must exist");
        assert_eq!(
            resolved_path.canonicalize().unwrap_or(resolved_path),
            expected,
            "JSON-path preset with no source_package must resolve at project root"
        );
    }
}
