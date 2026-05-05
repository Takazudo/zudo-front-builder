//! Content collection schema + walker.
//!
//! Mirrors Astro / zudo-doc style content collections: a typed directory of
//! markdown / MDX / TSX files with frontmatter that conforms to a schema.
//! Build-time consumers call [`walk_collection`] to get validated entries.
//! We also emit a `.zfb/types.d.ts` declaration so the TS rendering layer
//! (Epic 3) gets strong typing for `getCollection<"blog">()`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::frontmatter::{self, FrontmatterError, UnifiedFrontmatter};
use crate::mdx_jsx_emit::{compile_mdx_to_jsx_module_cached, CompiledMdx, MdxModuleCache};
use crate::pipeline::{Pipeline, PipelineError};
use crate::schema::json_schema_to_ts;

/// Source kind for an [`Entry`]. Discriminator on the union of file
/// shapes a collection may contain — used by downstream consumers (e.g.
/// the renderer) to decide whether to compile the body via the MDX path
/// or treat the source as a TSX module directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Authored as `.md` or `.mdx`. Body has been stripped of YAML
    /// frontmatter and compiled through the MDX→JSX emitter.
    Markdown,
    /// Authored as `.tsx`. The source IS the page module —
    /// [`Entry::compiled_jsx_source`] is the original TSX text and
    /// [`Entry::body`] is empty (TSX has no separate markdown body).
    Tsx,
}

/// One file in a collection.
///
/// Beyond the validated frontmatter, each entry carries the metadata the
/// JS-side renderer needs to mount it as a component: a stable
/// [`module_specifier`](Self::module_specifier) the loader can route on,
/// and the [`compiled_jsx_source`](Self::compiled_jsx_source) SWC
/// compiles into executable JS. Both are populated up-front during
/// [`walk_collection`] so renderers see a uniform shape regardless of
/// source extension.
///
/// For `.md` / `.mdx` entries (`kind == EntryKind::Markdown`), the body
/// is the markdown text after frontmatter stripping and the JSX source
/// is the MDX→JSX emitter output. For `.tsx` entries
/// (`kind == EntryKind::Tsx`), the body is empty (no markdown) and the
/// JSX source is the raw TSX text — SWC accepts TSX directly so no
/// pre-compile step is needed.
#[derive(Debug, Clone)]
pub struct Entry<T> {
    /// Slug from filename (no extension), e.g. "my-post" from "my-post.md".
    pub slug: String,
    /// Absolute path to the source file.
    pub path: PathBuf,
    /// Path relative to the collection root.
    pub rel_path: PathBuf,
    /// Validated, typed frontmatter.
    pub data: T,
    /// Markdown body (frontmatter stripped). Empty string for TSX
    /// entries, which have no separate markdown body.
    pub body: String,
    /// Stable specifier used by the renderer to address this entry's
    /// compiled module. Format depends on [`Self::kind`]:
    ///
    /// - `Markdown` → `mdx://<collection>/<slug>#<hash8>`, hash derived
    ///   from [`Self::compiled_jsx_source`].
    /// - `Tsx` → `tsx://<collection>/<slug>#<hash8>`, hash derived from
    ///   [`Self::compiled_jsx_source`] (which IS the raw TSX source).
    pub module_specifier: String,
    /// JSX module source for this entry, ready to feed into SWC. For
    /// `.md` / `.mdx` this is the MDX emitter output. For `.tsx` this
    /// is the raw source — SWC handles TSX without a pre-compile step.
    pub compiled_jsx_source: String,
    /// Discriminator for the source kind.
    pub kind: EntryKind,
    /// `export const extension = "…"` literal, when present. Always
    /// `None` for `Markdown` entries; populated from the TSX export
    /// when present for `Tsx` entries.
    pub extension: Option<String>,
    /// `export const contentType = "…"` literal, when present. Always
    /// `None` for `Markdown` entries; populated from the TSX export
    /// when present for `Tsx` entries.
    pub content_type: Option<String>,
}

impl<T> Entry<T> {
    /// Compile this entry's body into a [`CompiledMdx`] (JSX source +
    /// content hash + `mdx://` specifier).
    ///
    /// Only meaningful for `Markdown` entries. For `Tsx` entries, the
    /// body is empty and recompiling produces a degenerate empty
    /// module — call sites should branch on [`Self::kind`] before
    /// invoking this.
    ///
    /// Pass `Some(&cache)` to dedupe identical bodies across calls; pass
    /// `None` for a one-shot compile.
    ///
    /// `pipeline` was added by Sub 46 (#46) so callers can thread a
    /// configured [`Pipeline`] (whose mdast visitors mutate the tree
    /// before JSX emission) through. Pass `None` to keep today's
    /// pre-Sub-46 behaviour.
    ///
    /// # Errors
    /// Forwards [`PipelineError::Parse`] from the MDX emitter.
    pub fn compile_mdx(
        &self,
        cache: Option<&MdxModuleCache>,
        pipeline: Option<&mut Pipeline>,
    ) -> Result<CompiledMdx, PipelineError> {
        compile_mdx_to_jsx_module_cached(&self.body, &self.path, cache, pipeline)
    }
}

/// Errors produced by the collection walker.
#[derive(Debug, thiserror::Error)]
pub enum CollectionError {
    #[error("io error reading {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("frontmatter error in {path}: {message}", path = path.display())]
    Frontmatter { path: PathBuf, message: String },
    #[error("schema validation failed in {path}: {report}", path = path.display())]
    Schema { path: PathBuf, report: String },
    #[error("MDX compile error in {path}: {message}", path = path.display())]
    Mdx { path: PathBuf, message: String },
    #[error("multiple errors:\n{summary}")]
    Multiple {
        summary: String,
        errors: Vec<CollectionError>,
    },
}

/// Walk a collection directory, parsing + validating each `.md`, `.mdx`,
/// or `.tsx` file.
///
/// - `dir`: collection root.
/// - Returns `Vec<Entry<T>>` sorted by `rel_path` for deterministic ordering.
/// - Skips files with other extensions silently.
/// - Aggregates errors: if any entry fails, returns `Err(CollectionError::Multiple { .. })`
///   with all failures (not just the first).
///
/// Equivalent to [`walk_collection_with_cache`] called with
/// `cache = None`, forwarding `pipeline` verbatim — every markdown entry's
/// MDX is compiled fresh. Long-running consumers (a dev server, a build
/// that touches the same collection multiple times) should prefer the
/// `_with_cache` variant so identical bodies are emitted once. The cache
/// is irrelevant for TSX entries (no MDX compile step).
///
/// `pipeline` was added by Sub 46 (#46); pass `None` to keep today's
/// pre-Sub-46 behaviour. See [`walk_collection_with_cache`] for the full
/// rationale.
pub fn walk_collection<T>(
    dir: &Path,
    pipeline: Option<&mut Pipeline>,
) -> Result<Vec<Entry<T>>, CollectionError>
where
    T: DeserializeOwned + garde::Validate<Context = ()>,
{
    walk_collection_with_cache(dir, None, pipeline)
}

/// Walk a collection directory like [`walk_collection`], reusing `cache`
/// to dedupe compiled MDX modules across markdown entries.
///
/// `cache: Some(&MdxModuleCache)` short-circuits compilation when an
/// entry's raw body has been seen before — useful when the same
/// collection is re-walked after a partial change (only the modified
/// file recompiles). `cache: None` recompiles every entry, matching
/// the simple [`walk_collection`] contract.
///
/// `.md` and `.mdx` files take the same MDX→JSX path (CommonMark is a
/// strict MDX subset, so one emitter handles both). `.tsx` files skip
/// the MDX compile step entirely — the source is already JSX-shaped and
/// the renderer feeds it straight to SWC. Each successful entry gets its
/// [`Entry::module_specifier`] and [`Entry::compiled_jsx_source`]
/// populated up-front so consumers see a uniform shape.
///
/// `pipeline` was added by Sub 46 (#46) so the JSX emit step can run a
/// configured [`Pipeline`]'s mdast visitors against each markdown entry's
/// body. All in-tree call sites pass `None` today, which keeps behaviour
/// byte-for-byte identical to pre-Sub-46. The pipeline is borrowed
/// mutably across the whole walk because its visitors take `&mut self`.
/// TSX entries skip the pipeline entirely (no mdast tree).
pub fn walk_collection_with_cache<T>(
    dir: &Path,
    cache: Option<&MdxModuleCache>,
    mut pipeline: Option<&mut Pipeline>,
) -> Result<Vec<Entry<T>>, CollectionError>
where
    T: DeserializeOwned + garde::Validate<Context = ()>,
{
    let mut files: Vec<PathBuf> = Vec::new();
    collect_collection_files(dir, &mut files).map_err(|e| CollectionError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    files.sort();

    let mut entries: Vec<Entry<T>> = Vec::new();
    let mut errors: Vec<CollectionError> = Vec::new();

    for path in files {
        // Reset per-document pipeline state (e.g. HeadingLinksPlugin's
        // slug-dedupe counter) before each new entry so cross-document
        // state cannot leak from one file to the next. This mirrors
        // the bundler's `materialise_collection` / `materialise_shadow`
        // walk loops in `crates/zfb-build/src/bundler.rs`. Without
        // this, the snapshot produced by `build_snapshot` and the
        // bundler's bridge map disagree on the JSX `content_hash` for
        // any file processed after a slug-colliding heading earlier in
        // the same walk — `bridge.get(spec)` then misses and the page
        // silently falls back to `<pre data-zfb-content-fallback>`.
        // Re-borrow the pipeline for each entry so the loop can use the
        // mutable reference across iterations without consuming it.
        if let Some(p) = pipeline.as_deref_mut() {
            p.reset_per_entry();
            // Per-file source_dir for ResolveLinksPlugin (no-op when
            // the plugin isn't wired). The bundler does the same in
            // `crates/zfb-build/src/bundler.rs`'s materialise_*
            // helpers — without this, relative `[link](./other.mdx)`
            // references in collection content are emitted as raw
            // markdown links instead of resolved URLs, which changes
            // the compiled JSX byte-for-byte and diverges the
            // snapshot's content_hash from the bundler's.
            if let Some(parent) = path.parent() {
                p.set_resolve_links_source_dir(parent.to_path_buf());
            }
        }
        match parse_entry::<T>(dir, &path, cache, pipeline.as_deref_mut()) {
            Ok(entry) => entries.push(entry),
            Err(e) => errors.push(e),
        }
    }

    if !errors.is_empty() {
        let summary = errors
            .iter()
            .map(|e| format!("- {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(CollectionError::Multiple { summary, errors });
    }

    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(entries)
}

/// Generate a TypeScript `.d.ts` file describing one or more collections.
///
/// Emits two `declare module` blocks side-by-side:
///
/// 1. `zfb-collections` — original v1 surface, one type per collection
///    plus a `getCollection` overload. Kept for backward compatibility
///    with existing build tooling.
/// 2. `zfb/content` — the bridge module backing the Sub 48 content
///    contract. Surfaces `getCollection` AND `getEntry`, both keyed by
///    a `ZfbCollections` interface. Each collection entry includes
///    `slug`, `data` (frontmatter as `Record<string, unknown>`), and
///    `body` so TSX page modules can render markdown bodies on demand.
///
/// Example output for `["blog", "docs"]`:
///
/// ```ts
/// declare module "zfb-collections" {
///   export interface Collections {
///     blog: { slug: string; data: Record<string, unknown> };
///     docs: { slug: string; data: Record<string, unknown> };
///   }
///   export function getCollection<K extends keyof Collections>(name: K): Collections[K][];
/// }
///
/// declare module "zfb/content" {
///   export interface ZfbCollections {
///     blog: { slug: string; data: Record<string, unknown>; body: string };
///     docs: { slug: string; data: Record<string, unknown>; body: string };
///   }
///   export function getCollection<K extends keyof ZfbCollections>(name: K): ZfbCollections[K][];
///   export function getEntry<K extends keyof ZfbCollections>(name: K, slug: string): ZfbCollections[K] | undefined;
/// }
/// ```
///
/// **Limitation (v1):** the per-collection `data` field is typed as
/// `Record<string, unknown>`. Full schema reflection (turning the Rust schema
/// `T` into a precise TypeScript interface) is a follow-up.
///
/// A representative TS file using the new surface lives at
/// `crates/zfb-content/tests/fixtures/sample.ts` — the
/// `dts_typechecks_against_sample_fixture` test asserts the textual
/// declarations referenced from there are present in the emitted
/// `.d.ts`. Running `tsc` against the pair would produce no errors.
pub fn emit_types_dts(out_path: &Path, collection_names: &[&str]) -> Result<(), CollectionError> {
    let collections: Vec<CollectionTypeInfo<'_>> = collection_names
        .iter()
        .map(|name| CollectionTypeInfo {
            name,
            schema: None,
        })
        .collect();
    emit_types_dts_with_schemas(out_path, &collections)
}

/// Per-collection input to [`emit_types_dts_with_schemas`].
///
/// `schema` is an optional JSON Schema describing the frontmatter shape
/// for this collection. When `None`, the emitter falls back to the v1
/// `data: Record<string, unknown>` shape — backward compatible with
/// projects that haven't declared a schema. When `Some`, the schema is
/// converted (via [`crate::schema::json_schema_to_ts`]) into a precise
/// inline TypeScript type.
///
/// Recognised JSON Schema dialect: `type` (string / number / integer /
/// boolean / array / object / null, or an array of these), `items`,
/// `properties`, `required`, `enum`. See
/// [`crate::schema`] for the full conversion rules.
#[derive(Debug, Clone)]
pub struct CollectionTypeInfo<'a> {
    pub name: &'a str,
    pub schema: Option<&'a JsonValue>,
}

/// Generate a TypeScript `.d.ts` file with PRECISE per-collection types.
///
/// Same overall shape as [`emit_types_dts`] (two `declare module`
/// blocks: `zfb-collections` and `zfb/content`), but the `data` field
/// for each collection is rendered from the supplied JSON Schema instead
/// of the catch-all `Record<string, unknown>`. Collections whose schema
/// is `None` keep the v1 `Record<string, unknown>` shape so the call
/// sites that have no schema yet stay green.
///
/// Example output (one schema-typed, one untyped):
///
/// ```ts
/// declare module "zfb/content" {
///   export interface ZfbCollections {
///     docs: { slug: string; data: { title: string; sidebar_position?: number }; body: string };
///     blog: { slug: string; data: Record<string, unknown>; body: string };
///   }
///   export function getCollection<K extends keyof ZfbCollections>(name: K): ZfbCollections[K][];
///   export function getEntry<K extends keyof ZfbCollections>(name: K, slug: string): ZfbCollections[K] | undefined;
/// }
/// ```
pub fn emit_types_dts_with_schemas(
    out_path: &Path,
    collections: &[CollectionTypeInfo<'_>],
) -> Result<(), CollectionError> {
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| CollectionError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }

    let mut buf = String::new();
    buf.push_str("// Auto-generated by zfb-content. Do not edit by hand.\n");

    // Legacy module — kept so existing zfb-collections consumers don't
    // break. Sub 48 (#48) added the second module below.
    buf.push_str("declare module \"zfb-collections\" {\n");
    buf.push_str("  export interface Collections {\n");
    for info in collections {
        let data_ts = data_type_for(info.schema, 4);
        buf.push_str(&format!(
            "    {name}: {{ slug: string; data: {data_ts} }};\n",
            name = info.name,
        ));
    }
    buf.push_str("  }\n");
    buf.push_str(
        "  export function getCollection<K extends keyof Collections>(name: K): Collections[K][];\n",
    );
    buf.push_str("}\n");

    // New `zfb/content` module — the bridge surface specified in #48.
    buf.push('\n');
    buf.push_str("declare module \"zfb/content\" {\n");
    buf.push_str("  export interface ZfbCollections {\n");
    for info in collections {
        let data_ts = data_type_for(info.schema, 4);
        buf.push_str(&format!(
            "    {name}: {{ slug: string; data: {data_ts}; body: string }};\n",
            name = info.name,
        ));
    }
    buf.push_str("  }\n");
    buf.push_str(
        "  export function getCollection<K extends keyof ZfbCollections>(name: K): ZfbCollections[K][];\n",
    );
    buf.push_str(
        "  export function getEntry<K extends keyof ZfbCollections>(name: K, slug: string): ZfbCollections[K] | undefined;\n",
    );
    buf.push_str("}\n");

    let mut file = fs::File::create(out_path).map_err(|e| CollectionError::Io {
        path: out_path.to_path_buf(),
        source: e,
    })?;
    file.write_all(buf.as_bytes())
        .map_err(|e| CollectionError::Io {
            path: out_path.to_path_buf(),
            source: e,
        })?;
    Ok(())
}

fn data_type_for(schema: Option<&JsonValue>, indent: usize) -> String {
    match schema {
        Some(s) => json_schema_to_ts(s, indent),
        None => "Record<string, unknown>".to_string(),
    }
}

// -----------------------------------------------------------------------------
// internals
// -----------------------------------------------------------------------------

fn collect_collection_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Skip dotfiles (e.g. `.DS_Store`, editor swap files,
        // `.git`-style shadow directories). Mirrors the implicit
        // behaviour of zfb-router's scan, which only matches files
        // ending in `.tsx` and was therefore already immune.
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        // `entry.file_type()` describes the dirent itself, not the
        // target — symlinks have `is_dir() == false` and
        // `is_file() == false`. We never recurse into them and never
        // accept them as collection entries, matching
        // `WalkDir::follow_links(false)` in zfb-router::scan.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_collection_files(&path, out)?;
        } else if file_type.is_file() && is_collection_entry(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_collection_entry(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("md") | Some("mdx") | Some("tsx")
    )
}

fn parse_entry<T>(
    root: &Path,
    path: &Path,
    cache: Option<&MdxModuleCache>,
    pipeline: Option<&mut Pipeline>,
) -> Result<Entry<T>, CollectionError>
where
    T: DeserializeOwned + garde::Validate<Context = ()>,
{
    let raw = fs::read_to_string(path).map_err(|e| CollectionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    // Dispatch to the unified frontmatter API. YAML for .md/.mdx,
    // `export const frontmatter` for .tsx — both normalised to JSON.
    let uf = frontmatter::extract(path, &raw).map_err(|e| frontmatter_to_collection_error(path, e))?;

    // serde_json::from_value clones the JSON into the typed schema. We
    // route through JSON instead of bypassing it (e.g. via untyped
    // YAML) so schema validation downstream sees the exact same value
    // shape regardless of source kind.
    let data: T = serde_json::from_value(uf.value.clone()).map_err(|e| {
        CollectionError::Frontmatter {
            path: path.to_path_buf(),
            message: e.to_string(),
        }
    })?;

    data.validate().map_err(|report| CollectionError::Schema {
        path: path.to_path_buf(),
        report: report.to_string(),
    })?;

    let rel_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    // Slug derivation: the path of the entry **relative to the
    // collection root**, with the file extension stripped and path
    // separators normalised to `/`. This gives a stable, unique
    // identifier even when sibling categories all have an
    // `index.{md,mdx,tsx}`. The previous `path.file_stem()` slug
    // collapsed every nested `index.mdx` to the literal string
    // `"index"`, producing duplicate snapshot entries and
    // ambiguous-URL collisions in user `paths()` expansions.
    let slug = {
        let lossy = rel_path.to_string_lossy();
        let posix = if std::path::MAIN_SEPARATOR == '/' {
            lossy.into_owned()
        } else {
            lossy.replace(std::path::MAIN_SEPARATOR, "/")
        };
        // Strip any single recognised content-file extension. We match
        // the trailing `.<ext>` against the small fixed allowlist of
        // extensions the collection walker accepts (md, mdx, tsx) so
        // arbitrary `.foo.bar.mdx` filenames keep their leading dots.
        let stripped = ["md", "mdx", "tsx"]
            .iter()
            .find_map(|ext| {
                let needle = format!(".{ext}");
                posix.strip_suffix(&needle).map(str::to_owned)
            })
            .unwrap_or(posix);
        stripped
    };

    let UnifiedFrontmatter {
        body,
        extension,
        content_type,
        ..
    } = uf;

    let (collection_seg, slug_seg) = collection_and_slug_from_path(path);

    let kind = entry_kind_from_path(path);
    let (body, compiled_jsx_source, module_specifier) = match kind {
        EntryKind::Markdown => {
            let md_body = body.unwrap_or_default();
            // Compile body to JSX (same path for `.md` and `.mdx` —
            // CommonMark is a strict MDX subset). The cache opt-in
            // means repeat walks of an unchanged corpus skip the
            // emitter entirely. Sub 46 (#46) plumbs an optional
            // [`Pipeline`] through; today's callers pass `None`.
            let compiled = compile_mdx_to_jsx_module_cached(&md_body, path, cache, pipeline)
                .map_err(|e| {
                CollectionError::Mdx {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                }
            })?;
            // Derive the specifier from THIS entry's file path + the
            // JSX hash rather than reusing `compiled.specifier`
            // directly. The cache from Sub 2 keys on `sha256(body)` and
            // returns the cached `CompiledMdx` verbatim — including
            // the specifier baked at first compile. Two entries with
            // byte-identical bodies but different filenames must still
            // each get a specifier whose `<slug>` segment matches their
            // own file. The hash component is invariant (same body →
            // same JSX → same hash), so it's safe to reuse.
            let specifier = format!(
                "mdx://{collection_seg}/{slug_seg}#{hash}",
                hash = compiled.content_hash,
            );
            (md_body, compiled.jsx_source, specifier)
        }
        EntryKind::Tsx => {
            // TSX is already JSX-shaped — SWC accepts it as-is. The
            // body field is empty by convention (no markdown body).
            // Specifier scheme is `tsx://` so the renderer can dispatch
            // on prefix without re-reading the source.
            let hash = hash_8(&raw);
            let specifier = format!("tsx://{collection_seg}/{slug_seg}#{hash}");
            (String::new(), raw.clone(), specifier)
        }
    };

    Ok(Entry {
        slug,
        path: path.to_path_buf(),
        rel_path,
        data,
        body,
        module_specifier,
        compiled_jsx_source,
        kind,
        extension,
        content_type,
    })
}

fn entry_kind_from_path(path: &Path) -> EntryKind {
    match path.extension().and_then(|s| s.to_str()) {
        Some("tsx") => EntryKind::Tsx,
        // Markdown is the default — `is_collection_entry` already
        // gated the walker so other extensions never reach here.
        _ => EntryKind::Markdown,
    }
}

/// Convert a [`FrontmatterError`] into the closest [`CollectionError`]
/// variant. YAML / TSX / unsupported-extension cases all collapse onto
/// `Frontmatter` — the walker's caller treats them as a "this file's
/// frontmatter could not be loaded" failure regardless of cause.
fn frontmatter_to_collection_error(path: &Path, err: FrontmatterError) -> CollectionError {
    CollectionError::Frontmatter {
        path: path.to_path_buf(),
        message: err.to_string(),
    }
}

/// First 8 lowercase-hex chars of `sha256(input)`. Mirrors the dialect
/// used by [`compile_mdx_to_jsx_module_cached`] so MDX and TSX
/// specifiers share one hashing convention.
fn hash_8(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

/// Derive the `(collection, slug)` segments of a module specifier from
/// a content-collection file path. Mirrors the convention documented on
/// `compile_mdx_to_jsx_module_cached`: the immediate parent directory's
/// name is the collection, the file stem is the slug. Falls back to
/// `"_"` for either segment when the path lacks a parent or stem so
/// the result stays a parseable specifier.
fn collection_and_slug_from_path(file_path: &Path) -> (String, String) {
    let collection = file_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("_")
        .to_string();
    let slug = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("_")
        .to_string();
    (collection, slug)
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Deserialize, garde::Validate)]
    struct TestSchema {
        #[garde(length(min = 1))]
        title: String,
    }

    /// Self-cleaning temp dir (no `tempfile` dep — kept std-only).
    struct TmpDir {
        path: PathBuf,
    }

    impl TmpDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "zfb-content-collection-{label}-{nanos}-{n}-{pid}",
                pid = std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create tmp dir");
            Self { path: dir }
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let p = self.path.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, contents).expect("write file");
            p
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn valid_md(title: &str) -> String {
        format!("---\ntitle: \"{title}\"\n---\nbody for {title}\n")
    }

    fn valid_tsx(title: &str) -> String {
        format!(
            "export const frontmatter = {{ title: '{title}' }};\n\
             export default function Page() {{ return null; }}\n"
        )
    }

    #[test]
    fn walk_empty_directory_returns_empty_vec() {
        let tmp = TmpDir::new("empty");
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn walk_skips_dotfiles_and_dotdirs() {
        // Round 3 regression: editor swap files (`.foo.md.swp`),
        // `.DS_Store`, and shadow directories like `.git/HEAD` must
        // not be parsed as collection entries.
        let tmp = TmpDir::new("dotfiles");
        tmp.write("ok.md", &valid_md("OK"));
        tmp.write(".hidden.md", &valid_md("Hidden"));
        tmp.write(".DS_Store", "noise");
        // A .md file living under a dotdir must also be skipped — we
        // never recurse into dot-prefixed directory entries.
        tmp.write(".git/HEAD.md", &valid_md("Shadow"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 1, "only ok.md should survive");
        assert_eq!(out[0].slug, "ok");
    }

    #[test]
    fn walk_one_valid_md_file() {
        let tmp = TmpDir::new("one-md");
        tmp.write("hello.md", &valid_md("Hello"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.slug, "hello");
        assert_eq!(e.data.title, "Hello");
        assert_eq!(e.rel_path, PathBuf::from("hello.md"));
        assert!(e.body.contains("body for Hello"));
        assert_eq!(e.kind, EntryKind::Markdown);
        assert!(e.extension.is_none());
        assert!(e.content_type.is_none());
        assert!(e.module_specifier.starts_with("mdx://"));
        assert!(e.path.is_absolute() || e.path.exists());
    }

    #[test]
    fn walk_one_mdx_file_same_handling() {
        let tmp = TmpDir::new("one-mdx");
        tmp.write("post.mdx", &valid_md("Post"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].slug, "post");
        assert_eq!(out[0].rel_path, PathBuf::from("post.mdx"));
        assert_eq!(out[0].kind, EntryKind::Markdown);
    }

    #[test]
    fn walk_one_valid_tsx_file() {
        let tmp = TmpDir::new("one-tsx");
        tmp.write("page.tsx", &valid_tsx("Tsx Page"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.slug, "page");
        assert_eq!(e.data.title, "Tsx Page");
        assert_eq!(e.rel_path, PathBuf::from("page.tsx"));
        assert_eq!(e.kind, EntryKind::Tsx);
        assert!(e.body.is_empty(), "tsx entries have no markdown body");
        assert!(
            e.compiled_jsx_source.contains("export const frontmatter"),
            "tsx source should be carried verbatim into compiled_jsx_source",
        );
        assert!(e.module_specifier.starts_with("tsx://"));
    }

    #[test]
    fn walk_tsx_with_extension_and_content_type_exports() {
        let tmp = TmpDir::new("tsx-ext-ct");
        tmp.write(
            "feed.tsx",
            "export const frontmatter = { title: 'Feed' };\n\
             export const extension = 'xml';\n\
             export const contentType = 'application/rss+xml';\n\
             export default function Feed() { return null; }\n",
        );
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.kind, EntryKind::Tsx);
        assert_eq!(e.extension.as_deref(), Some("xml"));
        assert_eq!(e.content_type.as_deref(), Some("application/rss+xml"));
    }

    #[test]
    fn walk_mixed_md_mdx_tsx_collection() {
        let tmp = TmpDir::new("mixed");
        tmp.write("a-md.md", &valid_md("Alpha"));
        tmp.write("b-mdx.mdx", &valid_md("Beta"));
        tmp.write("c-tsx.tsx", &valid_tsx("Gamma"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 3, "all three extensions should walk");
        let titles: Vec<&str> = out.iter().map(|e| e.data.title.as_str()).collect();
        assert!(titles.contains(&"Alpha"));
        assert!(titles.contains(&"Beta"));
        assert!(titles.contains(&"Gamma"));
        // Schema validation (garde length(min=1)) ran against the
        // normalized JSON for every kind.
        for e in &out {
            assert!(!e.data.title.is_empty());
        }
    }

    #[test]
    fn walk_recursive_returns_sorted_entries() {
        let tmp = TmpDir::new("recursive");
        tmp.write("b.md", &valid_md("B"));
        tmp.write("nested/a.md", &valid_md("A"));
        tmp.write("nested/deep/c.mdx", &valid_md("C"));
        tmp.write("nested/deep/d.tsx", &valid_tsx("D"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 4);
        let rels: Vec<_> = out.iter().map(|e| e.rel_path.clone()).collect();
        let mut sorted = rels.clone();
        sorted.sort();
        assert_eq!(rels, sorted, "entries should be sorted by rel_path");
        assert!(rels.contains(&PathBuf::from("b.md")));
        assert!(rels.contains(&PathBuf::from("nested/a.md")));
        assert!(rels.contains(&PathBuf::from("nested/deep/c.mdx")));
        assert!(rels.contains(&PathBuf::from("nested/deep/d.tsx")));
    }

    #[test]
    fn walk_skips_unrelated_extensions() {
        let tmp = TmpDir::new("skips");
        tmp.write("post.md", &valid_md("Post"));
        tmp.write("README.txt", "just text");
        tmp.write("image.png", "not really png");
        tmp.write("notes.markdown", "wrong extension");
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path(), None).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].slug, "post");
    }

    #[test]
    fn malformed_frontmatter_yields_frontmatter_error() {
        let tmp = TmpDir::new("bad-yaml");
        // Open fence + intentionally broken YAML + close fence.
        tmp.write("bad.md", "---\ntitle: [unclosed\n---\nbody\n");
        let err = walk_collection::<TestSchema>(tmp.path(), None).unwrap_err();
        match err {
            CollectionError::Multiple { errors, .. } => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], CollectionError::Frontmatter { .. }));
            }
            other => unreachable!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn malformed_tsx_frontmatter_yields_frontmatter_error() {
        let tmp = TmpDir::new("bad-tsx");
        // Missing `export const frontmatter` → TsxFrontmatterError flows
        // through extract → CollectionError::Frontmatter.
        tmp.write(
            "bad.tsx",
            "export default function Page() { return null; }\n",
        );
        let err = walk_collection::<TestSchema>(tmp.path(), None).unwrap_err();
        match err {
            CollectionError::Multiple { errors, .. } => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], CollectionError::Frontmatter { .. }));
            }
            other => unreachable!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn schema_validation_failure_yields_schema_error() {
        let tmp = TmpDir::new("bad-schema");
        // Empty title violates length(min=1).
        tmp.write("empty.md", "---\ntitle: \"\"\n---\nbody\n");
        let err = walk_collection::<TestSchema>(tmp.path(), None).unwrap_err();
        match err {
            CollectionError::Multiple { errors, .. } => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], CollectionError::Schema { .. }));
            }
            other => unreachable!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn schema_validation_runs_against_tsx_entry() {
        let tmp = TmpDir::new("tsx-schema");
        // Empty title (string literal) violates garde length(min=1)
        // even though it parsed cleanly out of the TSX AST. Confirms
        // that validation operates on the normalized JSON regardless
        // of source kind.
        tmp.write(
            "empty.tsx",
            "export const frontmatter = { title: '' };\n\
             export default function Page() { return null; }\n",
        );
        let err = walk_collection::<TestSchema>(tmp.path(), None).unwrap_err();
        match err {
            CollectionError::Multiple { errors, .. } => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], CollectionError::Schema { .. }));
            }
            other => unreachable!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn multiple_failures_are_aggregated() {
        let tmp = TmpDir::new("aggregate");
        tmp.write("ok.md", &valid_md("OK"));
        tmp.write("bad-yaml.md", "---\ntitle: [unclosed\n---\n");
        tmp.write("bad-schema.md", "---\ntitle: \"\"\n---\nbody\n");
        let err = walk_collection::<TestSchema>(tmp.path(), None).unwrap_err();
        match err {
            CollectionError::Multiple { errors, summary } => {
                assert_eq!(errors.len(), 2, "expected 2 aggregated errors");
                let kinds: Vec<&str> = errors
                    .iter()
                    .map(|e| match e {
                        CollectionError::Frontmatter { .. } => "frontmatter",
                        CollectionError::Schema { .. } => "schema",
                        CollectionError::Io { .. } => "io",
                        CollectionError::Mdx { .. } => "mdx",
                        CollectionError::Multiple { .. } => "multiple",
                    })
                    .collect();
                assert!(kinds.contains(&"frontmatter"));
                assert!(kinds.contains(&"schema"));
                assert!(!summary.is_empty());
            }
            other => unreachable!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn emit_types_dts_writes_expected_module() {
        let tmp = TmpDir::new("dts");
        let out = tmp.path().join(".zfb").join("types.d.ts");
        emit_types_dts(&out, &["blog", "docs"]).unwrap();
        let s = fs::read_to_string(&out).unwrap();
        // Legacy module declarations (preserved for backward compat).
        assert!(s.contains("declare module \"zfb-collections\""));
        assert!(s.contains("blog:"));
        assert!(s.contains("docs:"));
        assert!(s.contains("getCollection"));
        // New zfb/content surface (Sub 48).
        assert!(s.contains("declare module \"zfb/content\""));
        assert!(s.contains("ZfbCollections"));
        assert!(s.contains("getEntry"));
        assert!(s.contains("body: string"));
    }

    /// Golden test: pin the exact textual output of `emit_types_dts` so
    /// any change to the declaration shape is loud. The string below is
    /// the canonical contract — when it changes, the bridge consumers
    /// (and the JS-side spec in adr-004) need to change with it.
    #[test]
    fn emit_types_dts_matches_golden() {
        let tmp = TmpDir::new("dts-golden");
        let out = tmp.path().join("types.d.ts");
        emit_types_dts(&out, &["blog", "docs"]).unwrap();
        let actual = fs::read_to_string(&out).unwrap();
        let expected = "// Auto-generated by zfb-content. Do not edit by hand.\n\
declare module \"zfb-collections\" {\n\
\x20\x20export interface Collections {\n\
\x20\x20\x20\x20blog: { slug: string; data: Record<string, unknown> };\n\
\x20\x20\x20\x20docs: { slug: string; data: Record<string, unknown> };\n\
\x20\x20}\n\
\x20\x20export function getCollection<K extends keyof Collections>(name: K): Collections[K][];\n\
}\n\
\n\
declare module \"zfb/content\" {\n\
\x20\x20export interface ZfbCollections {\n\
\x20\x20\x20\x20blog: { slug: string; data: Record<string, unknown>; body: string };\n\
\x20\x20\x20\x20docs: { slug: string; data: Record<string, unknown>; body: string };\n\
\x20\x20}\n\
\x20\x20export function getCollection<K extends keyof ZfbCollections>(name: K): ZfbCollections[K][];\n\
\x20\x20export function getEntry<K extends keyof ZfbCollections>(name: K, slug: string): ZfbCollections[K] | undefined;\n\
}\n";
        assert_eq!(
            actual, expected,
            "emit_types_dts output drifted from the golden contract",
        );
    }

    /// The sample.ts fixture exercises the full bridge surface using
    /// only types declared in the emitted `.d.ts`. We do not invoke
    /// `tsc` here (no Node toolchain in `cargo test`); instead, we
    /// assert the symbols the fixture references appear in the
    /// generated `.d.ts` text. If any of these strings drift, the
    /// fixture would stop typechecking — keeping this assertion green
    /// is the easy way to catch that without an external toolchain.
    #[test]
    fn dts_typechecks_against_sample_fixture() {
        let tmp = TmpDir::new("dts-sample");
        let out = tmp.path().join("types.d.ts");
        emit_types_dts(&out, &["blog"]).unwrap();
        let dts = fs::read_to_string(&out).unwrap();

        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample.ts");
        assert!(
            sample_path.exists(),
            "fixture not found at {}",
            sample_path.display(),
        );
        let sample = fs::read_to_string(&sample_path).unwrap();

        // Symbols the fixture imports / references must all be declared
        // in the generated d.ts.
        for needle in [
            "declare module \"zfb/content\"",
            "export function getCollection",
            "export function getEntry",
            "blog: { slug: string; data: Record<string, unknown>; body: string };",
        ] {
            assert!(
                dts.contains(needle),
                "expected {needle:?} in emitted d.ts:\n{dts}",
            );
        }

        // And the fixture must actually exercise these symbols.
        for needle in ["from \"zfb/content\"", "getCollection(", "getEntry("] {
            assert!(
                sample.contains(needle),
                "expected {needle:?} in sample.ts:\n{sample}",
            );
        }
    }

    /// `emit_types_dts_with_schemas` emits per-collection precise
    /// interfaces when a JSON Schema is supplied, and falls back to
    /// `Record<string, unknown>` when it isn't.
    #[test]
    fn emit_types_dts_with_schemas_produces_precise_data_field() {
        let tmp = TmpDir::new("dts-precise");
        let out = tmp.path().join("types.d.ts");

        let docs_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "sidebar_position": { "type": "number" },
                "draft": { "type": "boolean" },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["title"]
        });
        let collections = vec![
            CollectionTypeInfo {
                name: "docs",
                schema: Some(&docs_schema),
            },
            CollectionTypeInfo {
                name: "blog",
                schema: None,
            },
        ];
        emit_types_dts_with_schemas(&out, &collections).unwrap();
        let s = fs::read_to_string(&out).unwrap();

        // docs: precise — title required, sidebar_position optional number.
        assert!(s.contains("title: string;"), "docs schema: {s}");
        assert!(
            s.contains("sidebar_position?: number;"),
            "docs schema (optional number): {s}",
        );
        assert!(s.contains("draft?: boolean;"), "docs schema (optional boolean): {s}");
        assert!(s.contains("tags?: string[];"), "docs schema (array): {s}");

        // blog: no schema → falls back to Record<string, unknown>.
        assert!(
            s.contains("blog: { slug: string; data: Record<string, unknown>; body: string };"),
            "blog should keep v1 shape: {s}",
        );

        // Both module surfaces still present.
        assert!(s.contains("declare module \"zfb-collections\""));
        assert!(s.contains("declare module \"zfb/content\""));
    }

    /// Snapshot test: pin the precise emitted `.d.ts` for the two-collection
    /// fixture used in the brief (`docs` + `blog`). This is the textual
    /// contract zudo-doc's port consumes.
    #[test]
    fn emit_types_dts_with_schemas_matches_golden() {
        let tmp = TmpDir::new("dts-precise-golden");
        let out = tmp.path().join("types.d.ts");

        let docs_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "sidebar_position": { "type": "number" }
            },
            "required": ["title"]
        });
        let blog_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "date": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["title", "date"]
        });
        let collections = vec![
            CollectionTypeInfo {
                name: "docs",
                schema: Some(&docs_schema),
            },
            CollectionTypeInfo {
                name: "blog",
                schema: Some(&blog_schema),
            },
        ];
        emit_types_dts_with_schemas(&out, &collections).unwrap();
        let actual = fs::read_to_string(&out).unwrap();

        // The interface line per collection is the contract; pin both.
        // serde_json's default `Map` is a `BTreeMap`, so property
        // iteration is alphabetical — that's deterministic across runs
        // and good enough for the golden contract. (If we ever want
        // document-order keys, flip on serde_json's `preserve_order`
        // feature; the data-shape contract here doesn't depend on it.)
        assert!(
            actual.contains(
                "docs: { slug: string; data: {\n      sidebar_position?: number;\n      title: string;\n    }; body: string };"
            ),
            "docs interface drifted:\n{actual}",
        );
        assert!(
            actual.contains(
                "blog: { slug: string; data: {\n      date: string;\n      tags?: string[];\n      title: string;\n    }; body: string };"
            ),
            "blog interface drifted:\n{actual}",
        );
    }
}
