//! Sub 4 acceptance tests: collection layer surfaces `module_specifier`
//! and `compiled_jsx_source` for both `.md` and `.mdx` entries.
//!
//! Covers (per #31 acceptance criteria):
//! - A directory of mixed `.md` and `.mdx` is enumerated; each entry
//!   exposes `slug`, `data`, `body`, AND new `module_specifier` +
//!   `compiled_jsx_source`.
//! - Plain `.md` produces the same shape as `.mdx` — verified by
//!   creating one of each with equivalent body content and asserting
//!   the only diff is the slug suffix derived from the filename.
//! - The cache opt-in dedupes compiled modules across entries that
//!   share a body.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use zfb_content::collection::{walk_collection, walk_collection_with_cache, Entry};
use zfb_content::{parse_mdx_specifier, MdxModuleCache};

#[derive(Debug, Deserialize, garde::Validate)]
struct PostSchema {
    #[garde(length(min = 1))]
    title: String,
}

fn valid_md(title: &str, body: &str) -> String {
    format!("---\ntitle: \"{title}\"\n---\n{body}")
}

#[test]
fn mixed_md_and_mdx_entries_each_carry_specifier_and_jsx_source() {
    let tmp = TmpDir::new("mixed-md-mdx");
    // Write into a parent directory named "blog" so the specifier's
    // `<collection>` segment is meaningful.
    let blog = tmp.path().join("blog");
    std::fs::create_dir_all(&blog).unwrap();
    let blog_dir = TmpDirRef(&blog);

    blog_dir.write("alpha.md", &valid_md("Alpha", "alpha body\n"));
    blog_dir.write("beta.mdx", &valid_md("Beta", "beta body\n"));

    let entries: Vec<Entry<PostSchema>> = walk_collection(&blog, None).unwrap();
    assert_eq!(entries.len(), 2);

    for entry in &entries {
        // Each surface field must be populated with the expected shape.
        assert!(!entry.slug.is_empty(), "slug must be populated");
        assert!(!entry.data.title.is_empty(), "data.title must be populated");
        assert!(!entry.body.is_empty(), "body must be populated");
        assert!(
            !entry.module_specifier.is_empty(),
            "module_specifier must be populated for {}",
            entry.slug
        );
        assert!(
            !entry.compiled_jsx_source.is_empty(),
            "compiled_jsx_source must be populated for {}",
            entry.slug
        );

        // Specifier round-trips through the parser from Sub 2.
        let parsed = parse_mdx_specifier(&entry.module_specifier)
            .expect("module_specifier must be a valid mdx:// URL");
        assert_eq!(parsed.collection, "blog");
        assert_eq!(parsed.slug, entry.slug);

        // The compiled JSX is the standard module shape from Sub 1.
        assert!(
            entry
                .compiled_jsx_source
                .contains("export default function MDXContent"),
            "compiled_jsx_source must look like a JSX module: got {}",
            entry.compiled_jsx_source
        );
    }
}

#[test]
fn md_and_mdx_with_equivalent_content_differ_only_in_slug() {
    let tmp = TmpDir::new("md-vs-mdx");
    let blog = tmp.path().join("blog");
    std::fs::create_dir_all(&blog).unwrap();
    let blog_dir = TmpDirRef(&blog);

    let body = "# heading\n\nshared body paragraph\n";
    blog_dir.write("same.md", &valid_md("Same", body));
    blog_dir.write("same.mdx", &valid_md("Same", body));

    let entries: Vec<Entry<PostSchema>> = walk_collection(&blog, None).unwrap();
    assert_eq!(entries.len(), 2);

    let md = entries
        .iter()
        .find(|e| e.rel_path == Path::new("same.md"))
        .unwrap();
    let mdx = entries
        .iter()
        .find(|e| e.rel_path == Path::new("same.mdx"))
        .unwrap();

    // CommonMark is a strict MDX subset, so the emitter output must be
    // byte-identical for equivalent input.
    assert_eq!(
        md.compiled_jsx_source, mdx.compiled_jsx_source,
        "equivalent .md and .mdx must emit identical JSX",
    );

    // Both entries share the same slug stem — `same` — so the specifiers
    // are byte-identical too. Sub 2's specifier convention is
    // `mdx://<collection>/<slug>#<hash>` and the slug comes from the file
    // stem (no extension). Equivalent bodies share the same hash.
    assert_eq!(
        md.module_specifier, mdx.module_specifier,
        "equivalent .md and .mdx with the same slug must share a specifier",
    );

    // And the slug field itself drops the extension uniformly.
    assert_eq!(md.slug, "same");
    assert_eq!(mdx.slug, "same");
}

#[test]
fn cache_opt_in_dedupes_identical_bodies_across_entries() {
    let tmp = TmpDir::new("cache-dedupe");
    let blog = tmp.path().join("blog");
    std::fs::create_dir_all(&blog).unwrap();
    let blog_dir = TmpDirRef(&blog);

    // Two entries with byte-identical bodies (only the title varies, but
    // the title lives in YAML frontmatter — stripped before compile).
    // After stripping frontmatter the two bodies are the same, so a
    // cache lookup on the second entry must hit.
    let body = "# shared\n\nsame body\n";
    blog_dir.write("a.md", &valid_md("A", body));
    blog_dir.write("b.mdx", &valid_md("B", body));

    let cache = MdxModuleCache::new();
    assert!(cache.is_empty());

    let entries: Vec<Entry<PostSchema>> =
        walk_collection_with_cache(&blog, Some(&cache), None).unwrap();
    assert_eq!(entries.len(), 2);

    // One unique compiled body → exactly one cache entry.
    assert_eq!(
        cache.len(),
        1,
        "two entries with identical bodies must share one cache slot",
    );

    // Both entries' compiled JSX is identical (cache hit returned the
    // cached value verbatim).
    assert_eq!(
        entries[0].compiled_jsx_source,
        entries[1].compiled_jsx_source
    );

    // Specifiers differ in slug suffix (a vs b) but share the content
    // hash. The walker re-derives the specifier from each entry's own
    // file path even on a cache hit, so the slug always matches the
    // file the entry came from — see `parse_entry` in collection.rs.
    let pa = parse_mdx_specifier(&entries[0].module_specifier).unwrap();
    let pb = parse_mdx_specifier(&entries[1].module_specifier).unwrap();
    assert_eq!(pa.content_hash, pb.content_hash);
    assert_eq!(pa.slug, "a");
    assert_eq!(pb.slug, "b");
    assert_eq!(pa.collection, "blog");
    assert_eq!(pb.collection, "blog");
}

// -----------------------------------------------------------------------------
// TmpDir helper (std-only — same shape used by sibling integration tests).
// -----------------------------------------------------------------------------

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
            "zfb-content-collection-mdx-{label}-{nanos}-{n}-{pid}",
            pid = std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        Self { path: dir }
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Borrowed-path writer — lets us write into a subdirectory of a `TmpDir`
/// (e.g. the "blog" collection subfolder) without owning the cleanup.
struct TmpDirRef<'a>(&'a Path);

impl<'a> TmpDirRef<'a> {
    fn write(&self, rel: &str, contents: &str) {
        let p = self.0.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&p, contents).expect("write file");
    }
}
