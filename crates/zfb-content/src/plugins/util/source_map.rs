//! Build a source-path → URL map for cross-collection markdown link
//! resolution.
//!
//! Used as the first pass for [`crate::plugins::ResolveLinksPlugin`]:
//! callers walk every documented collection, build the map, then hand
//! it to the plugin which rewrites `.md` / `.mdx` link targets to the
//! corresponding site URL.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One collection in the source map.
///
/// `dir` is the on-disk directory holding the `.md` / `.mdx` files.
/// `route_prefix` is prepended to each file's slug to form the final
/// URL — typically `/docs/`, `/blog/`, etc. (include the trailing
/// slash; the slug is appended verbatim and a trailing `/` is added
/// after).
#[derive(Debug, Clone)]
pub struct CollectionRoute {
    /// Collection name (informational; not used for URL building).
    pub name: String,
    /// On-disk root directory for this collection.
    pub dir: PathBuf,
    /// Route prefix, e.g. `"/docs/"`.
    pub route_prefix: String,
}

/// Options passed to [`build_docs_source_map`].
#[derive(Debug, Clone, Default)]
pub struct DocsSourceMapOptions {
    /// Collections to scan, in order.
    pub collections: Vec<CollectionRoute>,
}

/// Walk every collection's `dir` and return a map of
/// `absolute_file_path` → `url_string`.
///
/// URL is `{route_prefix}{slug}/`, where `slug` is the file path
/// relative to `route.dir` with the `.md` / `.mdx` extension stripped.
/// Nested directories are preserved — `components/code-blocks.mdx`
/// becomes `{route_prefix}components/code-blocks/`, not
/// `{route_prefix}code-blocks/` (sub #234 nested-route fix). A trailing
/// `/index` segment is dropped so directory-style URLs collapse to the
/// parent (`guides/index.mdx` → `{route_prefix}guides/`,
/// `index.mdx` → `{route_prefix}`). Non-`.md` / `.mdx` files are
/// ignored.
///
/// Missing directories are silently skipped (a collection with no
/// content yet is not an error). I/O errors during traversal cause the
/// directory to be skipped — this helper is best-effort.
#[must_use]
pub fn build_docs_source_map(options: DocsSourceMapOptions) -> HashMap<PathBuf, String> {
    let mut map = HashMap::new();
    for route in &options.collections {
        if !route.dir.exists() {
            continue;
        }
        let mut files: Vec<PathBuf> = Vec::new();
        let _ = collect_md_files(&route.dir, &mut files);
        for path in files {
            let Some(slug) = slug_for(&route.dir, &path) else {
                continue;
            };
            let url = if slug.is_empty() {
                // `{route.dir}/index.mdx` → bare prefix (e.g. `/docs/`).
                route.route_prefix.clone()
            } else {
                format!("{}{}/", route.route_prefix, slug)
            };
            map.insert(path, url);
        }
    }
    map
}

/// Build the slug portion of the URL from `path`'s position under `dir`.
///
/// - Strips the `dir` prefix → keeps the nested directory structure.
/// - Strips the `.md` / `.mdx` extension.
/// - Drops a trailing `/index` segment so directory-style URLs collapse
///   to the parent (`guides/index.mdx` → `guides`,
///   `index.mdx` → `""` which the caller maps to the bare prefix).
/// - Normalises platform path separators to `/`.
///
/// Returns `None` when the path can't be expressed relative to `dir`
/// (caller skips such entries).
fn slug_for(dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?;
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = s.strip_suffix(".mdx") {
        s = stripped.to_string();
    } else if let Some(stripped) = s.strip_suffix(".md") {
        s = stripped.to_string();
    }
    if s == "index" {
        return Some(String::new());
    }
    if let Some(stripped) = s.strip_suffix("/index") {
        s = stripped.to_string();
    }
    Some(s)
}

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

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
                "zfb-content-source-map-{label}-{nanos}-{n}-{pid}",
                pid = std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create tmp dir");
            Self { path: dir }
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

    #[test]
    fn empty_options_returns_empty_map() {
        let map = build_docs_source_map(DocsSourceMapOptions::default());
        assert!(map.is_empty());
    }

    #[test]
    fn missing_dir_is_skipped() {
        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![CollectionRoute {
                name: "docs".into(),
                dir: PathBuf::from("/no/such/dir/xyz"),
                route_prefix: "/docs/".into(),
            }],
        });
        assert!(map.is_empty());
    }

    #[test]
    fn maps_md_and_mdx_files_to_urls() {
        let tmp = TmpDir::new("basic");
        let a = tmp.write("intro.md", "");
        let b = tmp.write("nested/advanced.mdx", "");
        tmp.write("ignore.txt", ""); // not md

        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![CollectionRoute {
                name: "docs".into(),
                dir: tmp.path.clone(),
                route_prefix: "/docs/".into(),
            }],
        });
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&a), Some(&"/docs/intro/".to_string()));
        // sub #234: nested directory survives — was `/docs/advanced/`
        // before the fix (file-stem flatten), broke 109 cross-page
        // links in zudolab/zudo-doc#1577.
        assert_eq!(map.get(&b), Some(&"/docs/nested/advanced/".to_string()));
    }

    #[test]
    fn deeply_nested_paths_preserve_full_directory_structure() {
        let tmp = TmpDir::new("deep");
        let a = tmp.write("guides/layout-demos/hide-toc.mdx", "");
        let b = tmp.write("components/basic-components.mdx", "");

        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![CollectionRoute {
                name: "docs".into(),
                dir: tmp.path.clone(),
                route_prefix: "/docs/".into(),
            }],
        });
        assert_eq!(
            map.get(&a),
            Some(&"/docs/guides/layout-demos/hide-toc/".to_string())
        );
        assert_eq!(
            map.get(&b),
            Some(&"/docs/components/basic-components/".to_string())
        );
    }

    #[test]
    fn nested_index_collapses_to_parent_directory_url() {
        let tmp = TmpDir::new("indexed");
        let a = tmp.write("guides/index.mdx", "");
        let b = tmp.write("components/index.md", "");

        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![CollectionRoute {
                name: "docs".into(),
                dir: tmp.path.clone(),
                route_prefix: "/docs/".into(),
            }],
        });
        assert_eq!(map.get(&a), Some(&"/docs/guides/".to_string()));
        assert_eq!(map.get(&b), Some(&"/docs/components/".to_string()));
    }

    #[test]
    fn root_index_maps_to_bare_route_prefix() {
        let tmp = TmpDir::new("root-index");
        let a = tmp.write("index.mdx", "");

        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![CollectionRoute {
                name: "docs".into(),
                dir: tmp.path.clone(),
                route_prefix: "/docs/".into(),
            }],
        });
        assert_eq!(map.get(&a), Some(&"/docs/".to_string()));
    }

    #[test]
    fn ja_locale_collection_uses_distinct_route_prefix() {
        // sub #234 multi-locale wiring: one collection per locale, each
        // with its own route_prefix. zudolab/zudo-doc#1577 surfaced this
        // when JA mirror docs would have resolved to `/docs/...` instead
        // of `/ja/docs/...` if the source map only knew one prefix.
        let en = TmpDir::new("en");
        let ja = TmpDir::new("ja");
        let en_file = en.write("getting-started/installation.mdx", "");
        let ja_file = ja.write("getting-started/installation.mdx", "");

        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![
                CollectionRoute {
                    name: "docs".into(),
                    dir: en.path.clone(),
                    route_prefix: "/docs/".into(),
                },
                CollectionRoute {
                    name: "docs-ja".into(),
                    dir: ja.path.clone(),
                    route_prefix: "/ja/docs/".into(),
                },
            ],
        });
        assert_eq!(
            map.get(&en_file),
            Some(&"/docs/getting-started/installation/".to_string())
        );
        assert_eq!(
            map.get(&ja_file),
            Some(&"/ja/docs/getting-started/installation/".to_string())
        );
    }

    #[test]
    fn multiple_collections_merge() {
        let blog = TmpDir::new("blog");
        let docs = TmpDir::new("docs");
        let p1 = blog.write("hello.md", "");
        let p2 = docs.write("guide.md", "");

        let map = build_docs_source_map(DocsSourceMapOptions {
            collections: vec![
                CollectionRoute {
                    name: "blog".into(),
                    dir: blog.path.clone(),
                    route_prefix: "/blog/".into(),
                },
                CollectionRoute {
                    name: "docs".into(),
                    dir: docs.path.clone(),
                    route_prefix: "/docs/".into(),
                },
            ],
        });
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&p1), Some(&"/blog/hello/".to_string()));
        assert_eq!(map.get(&p2), Some(&"/docs/guide/".to_string()));
    }
}
