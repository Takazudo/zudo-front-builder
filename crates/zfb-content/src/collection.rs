//! Content collection schema + walker.
//!
//! Mirrors Astro / zudo-doc style content collections: a typed directory of
//! markdown / MDX files with frontmatter that conforms to a schema. Build-time
//! consumers call [`walk_collection`] to get validated entries. We also emit a
//! `.zfb/types.d.ts` declaration so the TS rendering layer (Epic 3) gets
//! strong typing for `getCollection<"blog">()`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::mdx_jsx_emit::{compile_mdx_to_jsx_module_cached, CompiledMdx, MdxModuleCache};
use crate::pipeline::PipelineError;

/// One file in a collection.
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
    /// Markdown body (frontmatter stripped).
    pub body: String,
}

impl<T> Entry<T> {
    /// Compile this entry's body into a [`CompiledMdx`] (JSX source +
    /// content hash + `mdx://` specifier).
    ///
    /// Sub 2 surfaces this on `Entry` so downstream code can reach the
    /// new return value without re-deriving file-path conventions. The
    /// walker itself does **not** call this — wiring `Content` through
    /// `getCollection()` is Sub 4's job; for now this is an opt-in
    /// helper that exists so Sub 4 has a stable seam to attach to.
    ///
    /// Pass `Some(&cache)` to dedupe identical bodies across calls; pass
    /// `None` for a one-shot compile.
    ///
    /// # Errors
    /// Forwards [`PipelineError::Parse`] from the MDX emitter.
    pub fn compile_mdx(
        &self,
        cache: Option<&MdxModuleCache>,
    ) -> Result<CompiledMdx, PipelineError> {
        compile_mdx_to_jsx_module_cached(&self.body, &self.path, cache)
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
    #[error("multiple errors:\n{summary}")]
    Multiple {
        summary: String,
        errors: Vec<CollectionError>,
    },
}

/// Walk a collection directory, parsing + validating each `.md` / `.mdx` file.
///
/// - `dir`: collection root.
/// - Returns `Vec<Entry<T>>` sorted by `rel_path` for deterministic ordering.
/// - Skips non-md/mdx files silently.
/// - Aggregates errors: if any entry fails, returns `Err(CollectionError::Multiple { .. })`
///   with all failures (not just the first).
pub fn walk_collection<T>(dir: &Path) -> Result<Vec<Entry<T>>, CollectionError>
where
    T: DeserializeOwned + garde::Validate<Context = ()>,
{
    let mut files: Vec<PathBuf> = Vec::new();
    collect_md_files(dir, &mut files).map_err(|e| CollectionError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    files.sort();

    let mut entries: Vec<Entry<T>> = Vec::new();
    let mut errors: Vec<CollectionError> = Vec::new();

    for path in files {
        match parse_entry::<T>(dir, &path) {
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
/// Emits a single declaration like:
///
/// ```ts
/// declare module "zfb-collections" {
///   export interface Collections {
///     blog: { slug: string; data: Record<string, unknown> };
///     docs: { slug: string; data: Record<string, unknown> };
///   }
///   export function getCollection<K extends keyof Collections>(name: K): Collections[K][];
/// }
/// ```
///
/// **Limitation (v1):** the per-collection `data` field is typed as
/// `Record<string, unknown>`. Full schema reflection (turning the Rust schema
/// `T` into a precise TypeScript interface) is a follow-up.
pub fn emit_types_dts(out_path: &Path, collection_names: &[&str]) -> Result<(), CollectionError> {
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
    buf.push_str("declare module \"zfb-collections\" {\n");
    buf.push_str("  export interface Collections {\n");
    for name in collection_names {
        buf.push_str(&format!(
            "    {name}: {{ slug: string; data: Record<string, unknown> }};\n"
        ));
    }
    buf.push_str("  }\n");
    buf.push_str(
        "  export function getCollection<K extends keyof Collections>(name: K): Collections[K][];\n",
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

// -----------------------------------------------------------------------------
// internals
// -----------------------------------------------------------------------------

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_md_files(&path, out)?;
        } else if file_type.is_file() && is_md_or_mdx(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_md_or_mdx(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("md") | Some("mdx")
    )
}

fn parse_entry<T>(root: &Path, path: &Path) -> Result<Entry<T>, CollectionError>
where
    T: DeserializeOwned + garde::Validate<Context = ()>,
{
    let raw = fs::read_to_string(path).map_err(|e| CollectionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (yaml, body) = split_frontmatter(&raw).map_err(|message| CollectionError::Frontmatter {
        path: path.to_path_buf(),
        message,
    })?;

    let data: T = serde_yaml::from_str(yaml).map_err(|e| CollectionError::Frontmatter {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;

    data.validate().map_err(|report| CollectionError::Schema {
        path: path.to_path_buf(),
        report: report.to_string(),
    })?;

    let slug = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let rel_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    Ok(Entry {
        slug,
        path: path.to_path_buf(),
        rel_path,
        data,
        body,
    })
}

/// Minimal YAML frontmatter splitter: returns `(yaml_block, body)`.
///
/// Recognises the standard `---\n<yaml>\n---\n` opening fence. If the file
/// does not start with a frontmatter fence, returns `("", whole_file)`. If
/// the opening fence is present but the closing fence is missing, returns
/// an error.
///
/// NOTE: kept private + minimal on purpose — Sub 2 will deliver
/// `crate::frontmatter::parse`. After Wave 2 merges, a follow-up could swap
/// to that shared parser.
fn split_frontmatter(input: &str) -> Result<(&str, String), String> {
    // Strip optional UTF-8 BOM.
    let s = input.strip_prefix('\u{feff}').unwrap_or(input);

    // Detect + skip past an opening "---\n" (or "---\r\n") fence. If the file
    // does not start with a frontmatter fence, treat the entire file as body
    // and return an empty YAML block.
    let after_open = if let Some(rest) = s.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = s.strip_prefix("---\r\n") {
        rest
    } else {
        return Ok(("", s.to_string()));
    };

    // Find a closing fence: a line that is exactly "---".
    // We scan line by line to locate it.
    let mut yaml_end: Option<usize> = None; // byte offset within after_open where yaml ends
    let mut body_start: Option<usize> = None; // byte offset within after_open where body starts
    let bytes = after_open.as_bytes();
    let mut line_start = 0usize;
    let mut i = 0usize;
    while i <= bytes.len() {
        let at_eol = i == bytes.len() || bytes[i] == b'\n';
        if at_eol {
            // Determine the line slice (excluding trailing \r if present).
            let mut line_end = i;
            if line_end > line_start && bytes[line_end - 1] == b'\r' {
                line_end -= 1;
            }
            let line = &after_open[line_start..line_end];
            if line == "---" {
                yaml_end = Some(line_start);
                // Body starts after this line's terminating newline (or end of input).
                body_start = Some(if i < bytes.len() { i + 1 } else { i });
                break;
            }
            line_start = i + 1;
        }
        i += 1;
    }

    let (yaml_end, body_start) = match (yaml_end, body_start) {
        (Some(a), Some(b)) => (a, b),
        _ => return Err("missing closing '---' fence for frontmatter".to_string()),
    };

    let yaml = &after_open[..yaml_end];
    let body = if body_start <= after_open.len() {
        after_open[body_start..].to_string()
    } else {
        String::new()
    };
    Ok((yaml, body))
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

    #[test]
    fn walk_empty_directory_returns_empty_vec() {
        let tmp = TmpDir::new("empty");
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path()).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn walk_one_valid_md_file() {
        let tmp = TmpDir::new("one-md");
        tmp.write("hello.md", &valid_md("Hello"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path()).unwrap();
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.slug, "hello");
        assert_eq!(e.data.title, "Hello");
        assert_eq!(e.rel_path, PathBuf::from("hello.md"));
        assert!(e.body.contains("body for Hello"));
        assert!(e.path.is_absolute() || e.path.exists());
    }

    #[test]
    fn walk_one_mdx_file_same_handling() {
        let tmp = TmpDir::new("one-mdx");
        tmp.write("post.mdx", &valid_md("Post"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].slug, "post");
        assert_eq!(out[0].rel_path, PathBuf::from("post.mdx"));
    }

    #[test]
    fn walk_recursive_returns_sorted_entries() {
        let tmp = TmpDir::new("recursive");
        tmp.write("b.md", &valid_md("B"));
        tmp.write("nested/a.md", &valid_md("A"));
        tmp.write("nested/deep/c.mdx", &valid_md("C"));
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path()).unwrap();
        assert_eq!(out.len(), 3);
        let rels: Vec<_> = out.iter().map(|e| e.rel_path.clone()).collect();
        let mut sorted = rels.clone();
        sorted.sort();
        assert_eq!(rels, sorted, "entries should be sorted by rel_path");
        // Sanity: the three files are all present.
        assert!(rels.contains(&PathBuf::from("b.md")));
        assert!(rels.contains(&PathBuf::from("nested/a.md")));
        assert!(rels.contains(&PathBuf::from("nested/deep/c.mdx")));
    }

    #[test]
    fn walk_skips_non_md_or_mdx_files() {
        let tmp = TmpDir::new("skips");
        tmp.write("post.md", &valid_md("Post"));
        tmp.write("README.txt", "just text");
        tmp.write("image.png", "not really png");
        tmp.write("notes.markdown", "wrong extension");
        let out: Vec<Entry<TestSchema>> = walk_collection(tmp.path()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].slug, "post");
    }

    #[test]
    fn malformed_frontmatter_yields_frontmatter_error() {
        let tmp = TmpDir::new("bad-yaml");
        // Open fence + intentionally broken YAML + close fence.
        tmp.write("bad.md", "---\ntitle: [unclosed\n---\nbody\n");
        let err = walk_collection::<TestSchema>(tmp.path()).unwrap_err();
        match err {
            CollectionError::Multiple { errors, .. } => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], CollectionError::Frontmatter { .. }));
            }
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn schema_validation_failure_yields_schema_error() {
        let tmp = TmpDir::new("bad-schema");
        // Empty title violates length(min=1).
        tmp.write("empty.md", "---\ntitle: \"\"\n---\nbody\n");
        let err = walk_collection::<TestSchema>(tmp.path()).unwrap_err();
        match err {
            CollectionError::Multiple { errors, .. } => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], CollectionError::Schema { .. }));
            }
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn multiple_failures_are_aggregated() {
        let tmp = TmpDir::new("aggregate");
        tmp.write("ok.md", &valid_md("OK"));
        tmp.write("bad-yaml.md", "---\ntitle: [unclosed\n---\n");
        tmp.write("bad-schema.md", "---\ntitle: \"\"\n---\nbody\n");
        let err = walk_collection::<TestSchema>(tmp.path()).unwrap_err();
        match err {
            CollectionError::Multiple { errors, summary } => {
                assert_eq!(errors.len(), 2, "expected 2 aggregated errors");
                let kinds: Vec<&str> = errors
                    .iter()
                    .map(|e| match e {
                        CollectionError::Frontmatter { .. } => "frontmatter",
                        CollectionError::Schema { .. } => "schema",
                        CollectionError::Io { .. } => "io",
                        CollectionError::Multiple { .. } => "multiple",
                    })
                    .collect();
                assert!(kinds.contains(&"frontmatter"));
                assert!(kinds.contains(&"schema"));
                assert!(!summary.is_empty());
            }
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn emit_types_dts_writes_expected_module() {
        let tmp = TmpDir::new("dts");
        let out = tmp.path().join(".zfb").join("types.d.ts");
        emit_types_dts(&out, &["blog", "docs"]).unwrap();
        let s = fs::read_to_string(&out).unwrap();
        assert!(s.contains("declare module \"zfb-collections\""));
        assert!(s.contains("blog:"));
        assert!(s.contains("docs:"));
        assert!(s.contains("getCollection"));
    }
}
