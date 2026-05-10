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
//!    (the same one zfb-islands uses), then evaluated by `node` to pull the
//!    default export back as JSON, which is fed into `serde_json::from_str`.
//!    **TS wins over JSON when both files are present** — the TS form is the
//!    canonical, recommended way to author a zfb config.
//! 2. `dir/zfb.config.json` — read + parse via `serde_json`. Used only when
//!    no `zfb.config.ts` is found.
//! 3. Neither present — return [`Config::default`].
//!
//! TS support requires `node` in `PATH` and the staged esbuild binary at
//! `crates/zfb/binaries/esbuild/esbuild` (or the path pointed at by the
//! `ZFB_ESBUILD_BIN` environment variable). `node` was already a hard
//! this module surfaces a clean error if `node` is missing (still needed
//! for esbuild, prettier, and other JS tooling).
//!
//! All produced configs pass [`validate`] before they are returned so
//! callers don't have to think about it.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{de, Deserialize, Deserializer, Serialize};
use tokio::process::Command;

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

    /// JSX framework runtime. Default: `Preact`.
    #[serde(default)]
    pub framework: Framework,

    /// Content collections.
    #[serde(default)]
    pub collections: Vec<CollectionDef>,

    /// Tailwind-specific config; absent = default behavior.
    #[serde(default)]
    pub tailwind: Option<TailwindConfig>,

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
    /// the mdast pipeline after `AdmonitionsPlugin` so author-written
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            out_dir: default_out_dir(),
            public_dir: default_public_dir(),
            host: None,
            port: None,
            framework: Framework::default(),
            collections: Vec::new(),
            tailwind: None,
            plugins: Vec::new(),
            adapter: None,
            strip_md_ext: false,
            base: None,
            code_highlight: None,
            resolve_markdown_links: None,
            trailing_slash: false,
        }
    }
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
}

/// Tailwind options. Empty by default (Tailwind enabled); users can flip
/// `enabled: false` to opt out.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TailwindConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Syntect-based code-highlight options.
///
/// Controls the built-in syntax-highlight theme applied to fenced code
/// blocks in MDX content. Theme names are syntect's built-in set:
/// `"base16-ocean.dark"` (default), `"base16-ocean.light"`,
/// `"InspiredGitHub"`, `"Solarized (dark)"`, `"Solarized (light)"`.
///
/// **Note:** These are NOT Shiki theme names. Names like `"dracula"` or
/// `"github-dark"` are not part of syntect's bundled set and will
/// produce an `unknown theme` error at build time.
///
/// Unknown theme names are rejected with a clear error rather than
/// silently falling back — this matches the behaviour of
/// [`zfb_content::syntect_highlight::Highlighter::set_default_theme`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeHighlightConfig {
    /// Syntect built-in theme name.  When absent the pipeline defaults to
    /// `"base16-ocean.dark"`.
    #[serde(default)]
    pub theme: Option<String>,
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
/// mdast pipeline after `AdmonitionsPlugin` so author-written
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
/// config path (no node subprocess runs there) and for synthetic
/// configs constructed in tests; the [`Config`] consumers that actually
/// load plugins (the build/dev orchestration) treat a `None`
/// `resolved_module` as "no plugin module — skip this entry".
///
/// Sub 3 / issue #108: this field is populated by
/// `crates/zfb/js/config-loader.mjs` when the config goes through the
/// TS load path. The loader emits a `{ config, plugins }` envelope
/// where `plugins[i]` is the absolute module specifier for
/// `config.plugins[i].name`.
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

/// JS payload run by `node` to dynamic-import the bundled config and emit
/// the default export as JSON on stdout. Embedded into the binary so we
/// don't have to ship a sidecar file at runtime.
const CONFIG_LOADER_MJS: &str = include_str!("../js/config-loader.mjs");

/// Stub for the `zfb/config` (and `@takazudo/zfb/config`) import that user
/// TS configs reach for. We alias both the unscoped bare form (`zfb/config`)
/// and the full npm-package form (`@takazudo/zfb/config`) to this stub at
/// esbuild time so either spelling works without installing the npm package.
const CONFIG_STUB_MJS: &str = include_str!("../js/zfb-config-stub.mjs");

/// Default location of the staged esbuild CLI, mirroring
/// `zfb_build::bundler::DEFAULT_ESBUILD_SLOT`. Resolved relative to the
/// process working directory; release packaging stages the real binary
/// here.
const DEFAULT_ESBUILD_SLOT: &str = "crates/zfb/binaries/esbuild/esbuild";

/// Knobs that tweak loader behaviour. Public so build/dev/preview can
/// thread an explicit esbuild override through if they ever need to;
/// `Default` is the production path.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Override the esbuild binary path. `None` falls back to
    /// `ZFB_ESBUILD_BIN`, then [`DEFAULT_ESBUILD_SLOT`].
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

/// Load and validate a config from `dir`.
///
/// See the module docs for the resolution order.
pub async fn load_from_dir(dir: &Path) -> Result<Config> {
    load_from_dir_with_options(dir, &LoadOptions::default()).await
}

/// Variant of [`load_from_dir`] with explicit knobs. Most callers want
/// [`load_from_dir`].
pub async fn load_from_dir_with_options(
    dir: &Path,
    opts: &LoadOptions,
) -> Result<Config> {
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
        // entries that name a path on disk; bare specifiers can't be
        // resolved without a node subprocess and stay `None` with a
        // user-facing warning so the surprise is visible.
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
/// node subprocess that the TS path uses, so without this helper every
/// plugin entry would have `resolved_module = None` and the plugin-host
/// filter (`commands::plugins::build_plugin_specs`) would drop them all
/// silently — both `preBuild` and `postBuild` hooks would never fire.
///
/// Behaviour mirrors the TS path (`config-loader.mjs`) for the cases we
/// can handle without a JS runtime:
///
/// - Plugin entries naming a relative path (`./` or `../` prefix) or an
///   absolute path are canonicalised against `dir` and converted to a
///   `file://` URL via [`url::Url::from_file_path`]. A missing file is a
///   hard error pointing at the path the user wrote so the failure is
///   self-explanatory.
/// - Bare specifiers (`@scope/pkg`, `pkg-name`) cannot be resolved here
///   without running node; they stay `None` and we emit a user-visible
///   warning so the silent-drop behaviour is at least announced. The
///   user's recovery path is to switch to `zfb.config.ts`.
fn resolve_json_plugin_modules(cfg: &mut Config, dir: &Path) -> Result<()> {
    for entry in cfg.plugins.iter_mut() {
        let name = entry.name.as_str();
        let is_relative = name.starts_with("./") || name.starts_with("../");
        let is_absolute = name.starts_with('/');
        if is_relative || is_absolute {
            let raw_path = if is_relative {
                dir.join(name)
            } else {
                PathBuf::from(name)
            };
            let canonical = raw_path.canonicalize().with_context(|| {
                format!(
                    "plugin {:?}: cannot resolve plugin file at {} \
                     (does the file exist?)",
                    name,
                    raw_path.display()
                )
            })?;
            let url = url::Url::from_file_path(&canonical).map_err(|()| {
                anyhow!(
                    "plugin {:?}: failed to convert {} to a file:// URL",
                    name,
                    canonical.display()
                )
            })?;
            entry.resolved_module = Some(url.into());
        } else {
            // Bare specifier — no way to resolve without node. Warn so
            // the surprising "all plugins ignored" behaviour from #211
            // is at least announced. resolved_module stays None and the
            // plugin-host filter will drop this entry the same way it
            // does for synthetic test configs.
            crate::output::warn(format!(
                "plugin {:?}: bare specifiers cannot be resolved by zfb.config.json; \
                 use a relative path (e.g. \"./node_modules/{name}/index.mjs\") or \
                 switch to zfb.config.ts so node can resolve it",
                name
            ));
        }
    }
    Ok(())
}

/// Load a single `zfb.config.ts` file: bundle it with esbuild, evaluate
/// it with node, parse the JSON envelope (`{ config, plugins }`) the
/// loader emits, and merge the resolved plugin module specifiers back
/// onto `Config.plugins[].resolved_module`.
async fn load_from_ts_file(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
) -> Result<Config> {
    let json = if let Some(canned) = opts.test_default_export_json.as_deref() {
        canned.to_string()
    } else {
        load_ts_via_subprocess(ts_path, dir, opts).await?
    };
    parse_loader_envelope(&json, ts_path)
}

/// Internal envelope shape emitted by `crates/zfb/js/config-loader.mjs`.
///
/// The TS-load subprocess writes `{"config": <user-default-export>,
/// "plugins": [<resolved-module-specifier>, …]}` so we get both halves
/// in one parse. Older callers that supply `test_default_export_json`
/// can still pass either the envelope shape or a bare config object —
/// the bare-config branch is kept for backwards-test-compat.
#[derive(Debug, Deserialize)]
struct LoaderEnvelope {
    config: Config,
    #[serde(default)]
    plugins: Vec<String>,
}

fn parse_loader_envelope(json: &str, ts_path: &Path) -> Result<Config> {
    // Try the envelope shape first.
    if let Ok(envelope) = serde_json::from_str::<LoaderEnvelope>(json) {
        let LoaderEnvelope {
            mut config,
            plugins: resolved,
        } = envelope;
        if !resolved.is_empty() && resolved.len() != config.plugins.len() {
            bail!(
                "{}: plugin resolution count mismatch (config has {} plugins, loader resolved {}); \
                 this indicates a bug in config-loader.mjs",
                ts_path.display(),
                config.plugins.len(),
                resolved.len()
            );
        }
        for (entry, resolved_specifier) in config.plugins.iter_mut().zip(resolved.into_iter()) {
            entry.resolved_module = Some(resolved_specifier);
        }
        return Ok(config);
    }
    // Backwards-compat: tests that pre-date Sub 3 supply the bare config
    // JSON directly via `test_default_export_json`. Accept that shape so
    // the existing test suite keeps working.
    let cfg: Config = serde_json::from_str(json).map_err(|e| {
        anyhow!(
            "{}: failed to parse the default export as zfb config JSON \
             (line {}, column {}): {}\n--- received ---\n{}",
            ts_path.display(),
            e.line(),
            e.column(),
            e,
            json
        )
    })?;
    Ok(cfg)
}

/// Resolve the esbuild binary path using the same precedence the
/// build-time bundler uses:
/// 1. explicit `LoadOptions::esbuild_binary` override
/// 2. `ZFB_ESBUILD_BIN` env
/// 3. embedded extraction from the `EMBEDDED_VENDOR` snapshot (sub #212 —
///    the consumer-friendly path: works on a machine that has no
///    `crates/zfb/binaries/` workspace dir)
/// 4. the staged slot under `crates/zfb/binaries/esbuild/` (in-workspace
///    dev fallback)
///
/// Returns the resolved path. Most callers should use
/// [`resolve_esbuild_binary_with_handle`] instead — the embedded tier
/// returns a [`tempfile::TempDir`] that must be kept alive for the
/// lifetime of the spawned subprocess. This thin wrapper drops the
/// handle eagerly and is therefore only safe when the caller is sure the
/// path it gets back will not be the embedded one (e.g. in tests where
/// `ZFB_ESBUILD_BIN` is set, or when `crates/zfb/binaries/esbuild/esbuild`
/// is known to exist).
#[allow(dead_code)]
fn resolve_esbuild_binary(opts: &LoadOptions) -> Result<PathBuf> {
    let (_handle, path) = resolve_esbuild_binary_with_handle(opts)?;
    Ok(path)
}

/// Variant of [`resolve_esbuild_binary`] that also returns the
/// [`tempfile::TempDir`] handle backing the embedded extraction tier.
/// The caller MUST hold the handle alive for as long as the returned
/// `PathBuf` is referenced by a running subprocess — dropping the handle
/// removes the tempdir and the binary along with it.
fn resolve_esbuild_binary_with_handle(
    opts: &LoadOptions,
) -> Result<(Option<tempfile::TempDir>, PathBuf)> {
    if let Some(p) = opts.esbuild_binary.as_deref() {
        if !p.exists() {
            bail!(
                "config loader: esbuild binary not found at explicit path {}",
                p.display()
            );
        }
        return Ok((None, p.to_path_buf()));
    }
    if let Some(env) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(env);
        if !p.exists() {
            bail!(
                "config loader: esbuild binary not found at ZFB_ESBUILD_BIN={}",
                p.display()
            );
        }
        return Ok((None, p));
    }
    // Embedded extraction tier (sub #212). We try this BEFORE the
    // workspace-relative slot so consumers running `zfb build` from a
    // project that doesn't ship `crates/zfb/binaries/` still resolve a
    // working binary. The TempDir is propagated to the caller so the
    // extracted file outlives the subprocess invocation.
    match crate::render_pipeline::embedded_binary("esbuild") {
        Ok((handle, path)) => return Ok((Some(handle), path)),
        Err(_embed_err) => {
            // Fall through to the workspace-relative slot. The embedded
            // path is the expected production resolution for cargo-installed
            // binaries; failure here is normal during in-workspace dev (and
            // the slot fallback below covers that).
        }
    }
    let slot = PathBuf::from(DEFAULT_ESBUILD_SLOT);
    if !slot.exists() {
        return Err(anyhow!(
            "config loader: esbuild binary not found at default slot {}. \
             Either set ZFB_ESBUILD_BIN to a usable esbuild CLI, or stage \
             the binary at the slot path. (See crates/zfb/binaries/esbuild/README.md.)",
            slot.display()
        ));
    }
    Ok((None, slot))
}

/// Run esbuild + node to compile `ts_path` to ESM and pull the default
/// export back as JSON.
async fn load_ts_via_subprocess(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
) -> Result<String> {
    // The TempDir handle (when the embedded extraction tier is taken) must
    // outlive every subprocess spawn below — esbuild and node both reference
    // the extracted binary path. Drop only happens at function return.
    let (_esbuild_embed_guard, esbuild) = resolve_esbuild_binary_with_handle(opts)?;

    // Stage the embedded helper scripts and esbuild output into a
    // tempdir that vanishes at the end of this function.
    let tmp = tempfile::Builder::new()
        .prefix("zfb-config-")
        .tempdir()
        .context("config loader: failed to allocate temp directory")?;
    let stub_path = tmp.path().join("zfb-config-stub.mjs");
    let loader_path = tmp.path().join("config-loader.mjs");
    let bundle_path = tmp.path().join("zfb-config-bundle.mjs");
    tokio::fs::write(&stub_path, CONFIG_STUB_MJS)
        .await
        .context("config loader: failed to stage zfb-config-stub.mjs")?;
    tokio::fs::write(&loader_path, CONFIG_LOADER_MJS)
        .await
        .context("config loader: failed to stage config-loader.mjs")?;

    // 1. Bundle the user's TS file. Run with `dir` as cwd so any
    //    relative imports the user wrote (e.g. `./constants`) resolve
    //    against the project root.
    //
    // Alias both the bare `zfb/config` form (the documented convention in
    // the zfb docs) and the full npm-package form `@takazudo/zfb/config`
    // (what users naturally write when they know the package is published
    // as `@takazudo/zfb`). Both spellings must work because the canonical
    // example (examples/basic-blog/zfb.config.ts) uses the scoped form.
    let alias_arg = format!("--alias:zfb/config={}", stub_path.display());
    let alias_scoped_arg = format!("--alias:@takazudo/zfb/config={}", stub_path.display());
    let outfile_arg = format!("--outfile={}", bundle_path.display());
    let mut cmd = Command::new(&esbuild);
    cmd.current_dir(dir);
    cmd.arg("--bundle");
    cmd.arg("--format=esm");
    cmd.arg("--platform=node");
    cmd.arg("--target=esnext");
    cmd.arg("--log-level=warning");
    cmd.arg(&alias_arg);
    cmd.arg(&alias_scoped_arg);
    cmd.arg(&outfile_arg);
    cmd.arg(ts_path);

    let esbuild_out = cmd.output().await.with_context(|| {
        format!(
            "config loader: failed to spawn esbuild at {}",
            esbuild.display()
        )
    })?;
    if !esbuild_out.status.success() {
        let stderr = String::from_utf8_lossy(&esbuild_out.stderr);
        bail!(
            "config loader: esbuild failed to bundle {} ({}): {}",
            ts_path.display(),
            esbuild_out.status,
            stderr.trim()
        );
    }

    // 2. Run node against the loader script to print JSON of the default
    //    export. Same cwd so any runtime resolution that escapes the
    //    bundle (it shouldn't) still anchors at the project root.
    let node_bin: OsString = opts
        .node_binary
        .clone()
        .unwrap_or_else(|| OsString::from("node"));
    let mut node_cmd = Command::new(&node_bin);
    node_cmd.current_dir(dir);
    node_cmd.arg(&loader_path);
    node_cmd.arg(&bundle_path);
    // Project root — the loader uses this for path-relative plugin
    // resolution and for bare-specifier `node_modules` lookup.
    node_cmd.arg(dir);

    let node_out = match node_cmd.output().await {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "config loader: `{}` was not found in PATH. zfb requires \
                 Node.js to load `zfb.config.ts` (and to run esbuild / prettier). \
                 Install Node.js — https://nodejs.org/ \
                 — or point zfb at a node binary by setting the `ZFB_NODE_BIN` \
                 env var on a future zfb release.",
                node_bin.to_string_lossy()
            );
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!(
                "config loader: failed to spawn `{}`",
                node_bin.to_string_lossy()
            )));
        }
    };
    if !node_out.status.success() {
        let stderr = String::from_utf8_lossy(&node_out.stderr);
        bail!(
            "config loader: node failed evaluating {} ({}): {}",
            ts_path.display(),
            node_out.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(node_out.stdout).map_err(|e| {
        // Echo the lossy form so the operator can see what node actually
        // emitted — much more useful than a bare "not valid UTF-8".
        let bytes = e.as_bytes();
        anyhow!(
            "config loader: node stdout for {} was not valid UTF-8 ({} bytes): {}",
            ts_path.display(),
            bytes.len(),
            String::from_utf8_lossy(bytes)
        )
    })?;
    Ok(stdout)
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
        ensure_path_in_root(&c.path, dir)
            .with_context(|| format!("collection {:?}", c.name))?;
    }
    if let Some(rml) = &cfg.resolve_markdown_links {
        if !rml.docs_dir.as_os_str().is_empty() {
            ensure_path_in_root(&rml.docs_dir, dir)
                .context("resolveMarkdownLinks.docsDir")?;
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
        let looks_absolute_url =
            trimmed.starts_with("http://") || trimmed.starts_with("https://");
        if !trimmed.is_empty() && !looks_absolute_url && !trimmed.starts_with('/') {
            bail!(
                "base {:?} must start with `/` (e.g. \"/pj/zudo-doc/\") or be an absolute URL",
                b
            );
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
                    bail!(
                        "path {:?} escapes the project root via `..`",
                        path
                    );
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
        let err = JsonSchema::try_from_value(v)
            .expect_err("unknown type should be rejected");
        assert!(err.contains("\"timestamp\""), "err: {err}");
        assert!(err.contains("not recognised"), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_unknown_type_in_array() {
        let v = serde_json::json!({ "type": ["string", "date"] });
        let err = JsonSchema::try_from_value(v)
            .expect_err("unknown type in array should be rejected");
        assert!(err.contains("\"date\""), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_non_string_in_type_array() {
        let v = serde_json::json!({ "type": ["string", 42] });
        let err = JsonSchema::try_from_value(v)
            .expect_err("non-string in type array should be rejected");
        assert!(err.contains("must contain strings"), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_non_object_type_field() {
        let v = serde_json::json!({ "type": true });
        let err = JsonSchema::try_from_value(v)
            .expect_err("boolean type field should be rejected");
        assert!(err.contains("must be a string or array"), "err: {err}");
    }

    #[test]
    fn json_schema_rejects_non_object_properties() {
        let v = serde_json::json!({ "type": "object", "properties": ["a", "b"] });
        let err = JsonSchema::try_from_value(v)
            .expect_err("array properties should be rejected");
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
        assert_eq!(
            asset_url_base_prefix(Some("/pj/zudo-doc/")),
            "/pj/zudo-doc"
        );
    }

    #[test]
    fn asset_url_base_prefix_subpath_without_trailing_slash_is_idempotent() {
        // Authors who omit the trailing slash get the same prefix as
        // those who include it.
        assert_eq!(
            asset_url_base_prefix(Some("/pj/zudo-doc")),
            "/pj/zudo-doc"
        );
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
        let tmp = TempDir::new().unwrap();
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
                { "name": "my-plugin", "options": { "level": 2 } }
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
        assert_eq!(
            cfg.tailwind,
            Some(TailwindConfig { enabled: false })
        );
        assert_eq!(cfg.plugins.len(), 1);
        assert_eq!(cfg.plugins[0].name, "my-plugin");
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
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject");
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
        assert!(
            msg.contains("duplicate collection name"),
            "msg: {msg}"
        );
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
        // Defensive guard — if config-loader.mjs ever drifts and emits
        // a plugins array that doesn't 1:1 match config.plugins, we
        // surface that as a clear error rather than silently dropping
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
        assert!(msg.contains("plugin resolution count mismatch"), "msg: {msg}");
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
        assert!(
            msg.contains("duplicate collection name"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn ts_wins_over_json_when_both_present() {
        // Both files present → TS wins. The JSON file is ignored and the
        // test override is used to inject the canned TS export JSON.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{"port": 5500}"#,
        )
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
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{"port": 5500}"#,
        )
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

    #[tokio::test]
    async fn json_plugin_bare_specifier_stays_unresolved_no_error() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{
                "plugins": [
                    { "name": "@takazudo/zfb-shell-rename" },
                    { "name": "some-bare-pkg" }
                ]
            }"#,
        )
        .await
        .unwrap();

        // Bare specifiers cannot be resolved without node; they must
        // not raise an error here (warning is emitted instead, but we
        // do not assert on stderr text — that's a UX detail).
        let cfg = load_from_dir(tmp.path())
            .await
            .expect("bare specifier in JSON config must not error");
        assert_eq!(cfg.plugins.len(), 2);
        assert!(
            cfg.plugins[0].resolved_module.is_none(),
            "bare specifier must stay None (got {:?})",
            cfg.plugins[0].resolved_module
        );
        assert!(
            cfg.plugins[1].resolved_module.is_none(),
            "bare specifier must stay None (got {:?})",
            cfg.plugins[1].resolved_module
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
        assert!(
            msg.contains("Node.js"),
            "msg should mention Node.js: {msg}"
        );
    }

    /// Sub #212 — when neither `LoadOptions::esbuild_binary` nor the
    /// `ZFB_ESBUILD_BIN` env var is set, the resolver falls through to the
    /// embedded extraction tier (the binary staged inside the zfb crate at
    /// build time). The returned path must exist and the function must
    /// hand back a TempDir handle so the caller can keep the extracted
    /// binary alive for the lifetime of the spawned subprocess.
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
        let opts = LoadOptions::default();
        let (handle, path) = resolve_esbuild_binary_with_handle(&opts)
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
        assert!(msg.contains("received"), "msg should echo the payload: {msg}");
    }

    /// Real subprocess flow: esbuild + node against the canonical
    /// `examples/basic-blog/zfb.config.ts` example. Gated behind
    /// `--include-ignored` because the staged esbuild slot is empty in
    /// CI today (see crates/zfb/binaries/esbuild/README.md) and the
    /// test will fail to find the binary. Run locally with
    /// `ZFB_ESBUILD_BIN=$(which esbuild) cargo test ts_real_subprocess
    /// --include-ignored -p zfb`.
    #[tokio::test]
    #[ignore = "requires real esbuild + node; opt in via --include-ignored"]
    async fn ts_real_subprocess_loads_basic_blog_ts() {
        // Locate the example file via CARGO_MANIFEST_DIR to be cwd-
        // independent.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let example_ts = manifest_dir
            .join("../../examples/basic-blog/zfb.config.ts")
            .canonicalize()
            .expect("example zfb.config.ts must exist");

        // Copy the file into a fresh tmp project root so the loader
        // takes the TS branch (no sibling JSON).
        let tmp = TempDir::new().unwrap();
        let dst = tmp.path().join("zfb.config.ts");
        tokio::fs::copy(&example_ts, &dst).await.unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("real subprocess load should succeed");
        // Spot-check fields the example pins.
        assert_eq!(cfg.framework, Framework::Preact);
        assert_eq!(
            cfg.tailwind,
            Some(TailwindConfig { enabled: true })
        );
        assert_eq!(cfg.collections.len(), 1);
        assert_eq!(cfg.collections[0].name, "blog");
        assert_eq!(
            cfg.collections[0].path,
            PathBuf::from("content/blog")
        );
    }
}
