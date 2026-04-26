//! Integration test: build a small fake graph that mirrors a realistic
//! Epic-3-shaped resolver output and exercise the dirty-page query for the
//! main edit / add / delete / global flows.
//!
//! The shape modelled here:
//!
//! ```text
//! pages/index.tsx     → [layouts/main.tsx, components/header.tsx, styles/global.css]
//! pages/blog.tsx      → [layouts/main.tsx, components/post-card.tsx, content/blog/hello.md]
//! pages/about.tsx     → [layouts/main.tsx]
//! pages/contact.tsx   → [layouts/contact.tsx, components/form.tsx]
//! ```
//!
//! Plus a `zfb.config.ts` registered as global.

use std::path::PathBuf;

use zfb_graph::{DepKind, DependencyGraph, DirtySet, PageDeps, PageId};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn pid(s: &str) -> PageId {
    PageId::new(p(s))
}

fn fixture() -> DependencyGraph {
    let mut g = DependencyGraph::from_pages([
        PageDeps::new(
            pid("/proj/pages/index.tsx"),
            vec![
                (p("/proj/layouts/main.tsx"), DepKind::Module),
                (p("/proj/components/header.tsx"), DepKind::Module),
                (p("/proj/styles/global.css"), DepKind::Style),
            ],
        ),
        PageDeps::new(
            pid("/proj/pages/blog.tsx"),
            vec![
                (p("/proj/layouts/main.tsx"), DepKind::Module),
                (p("/proj/components/post-card.tsx"), DepKind::Module),
                (p("/proj/content/blog/hello.md"), DepKind::Content),
            ],
        ),
        PageDeps::new(
            pid("/proj/pages/about.tsx"),
            vec![(p("/proj/layouts/main.tsx"), DepKind::Module)],
        ),
        PageDeps::new(
            pid("/proj/pages/contact.tsx"),
            vec![
                (p("/proj/layouts/contact.tsx"), DepKind::Module),
                (p("/proj/components/form.tsx"), DepKind::Module),
            ],
        ),
    ]);
    g.mark_global("/proj/zfb.config.ts");
    g
}

#[test]
fn editing_a_unique_markdown_dirties_only_one_page() {
    let g = fixture();
    let d = g.dirty_pages(&p("/proj/content/blog/hello.md"));
    assert_eq!(d.as_pages().unwrap(), vec![pid("/proj/pages/blog.tsx")]);
}

#[test]
fn editing_the_shared_layout_dirties_three_pages() {
    let g = fixture();
    let d = g.dirty_pages(&p("/proj/layouts/main.tsx"));
    assert_eq!(
        d.as_pages().unwrap(),
        vec![
            pid("/proj/pages/about.tsx"),
            pid("/proj/pages/blog.tsx"),
            pid("/proj/pages/index.tsx"),
        ]
    );
}

#[test]
fn editing_a_page_source_dirties_just_that_page() {
    let g = fixture();
    let d = g.dirty_pages(&p("/proj/pages/contact.tsx"));
    assert_eq!(d.as_pages().unwrap(), vec![pid("/proj/pages/contact.tsx")]);
}

#[test]
fn editing_an_unrelated_dep_dirties_nothing() {
    let g = fixture();
    let d = g.dirty_pages(&p("/proj/pages/contact.tsx"));
    // Sanity: contact has no overlap with the index/blog/about cluster.
    let pages = d.as_pages().unwrap();
    assert!(!pages.contains(&pid("/proj/pages/index.tsx")));
    assert!(!pages.contains(&pid("/proj/pages/blog.tsx")));
    assert!(!pages.contains(&pid("/proj/pages/about.tsx")));
}

#[test]
fn config_change_returns_all_sentinel() {
    let g = fixture();
    let d = g.dirty_pages(&p("/proj/zfb.config.ts"));
    assert!(d.is_all());
    assert!(matches!(d, DirtySet::All));
}

#[test]
fn deleting_a_shared_dep_invalidates_all_consumers() {
    let mut g = fixture();
    let removed = g.remove_node(&p("/proj/layouts/main.tsx"));
    let removed: Vec<PageId> = removed.into_iter().collect();
    assert_eq!(
        removed,
        vec![
            pid("/proj/pages/about.tsx"),
            pid("/proj/pages/blog.tsx"),
            pid("/proj/pages/index.tsx"),
        ]
    );
    // After removal, dirtying that path again finds no consumers.
    let d = g.dirty_pages(&p("/proj/layouts/main.tsx"));
    assert!(d.as_pages().unwrap().is_empty());
}

#[test]
fn deleting_a_page_drops_it_and_cleans_reverse_index() {
    let mut g = fixture();
    let removed = g.remove_node(&p("/proj/pages/blog.tsx"));
    assert!(removed.contains(&pid("/proj/pages/blog.tsx")));
    assert_eq!(g.page_count(), 3);

    // The blog-only deps should no longer point to anyone.
    let d = g.dirty_pages(&p("/proj/content/blog/hello.md"));
    assert!(d.as_pages().unwrap().is_empty());

    let d = g.dirty_pages(&p("/proj/components/post-card.tsx"));
    assert!(d.as_pages().unwrap().is_empty());

    // But the shared layout is still wired to the other two pages.
    let d = g.dirty_pages(&p("/proj/layouts/main.tsx"));
    assert_eq!(
        d.as_pages().unwrap(),
        vec![pid("/proj/pages/about.tsx"), pid("/proj/pages/index.tsx")]
    );
}

#[test]
fn adding_a_brand_new_dep_then_resolving_records_consumer() {
    let mut g = fixture();
    // Filesystem watcher reports a new file. Before the resolver re-runs,
    // dirty_pages for that path is empty (no consumer yet).
    assert!(g.add_node("/proj/components/new-widget.tsx"));
    let d = g.dirty_pages(&p("/proj/components/new-widget.tsx"));
    assert!(d.as_pages().unwrap().is_empty());

    // Now the resolver re-resolves /proj/pages/index.tsx and reports the new
    // dep. After upsert, dirty_pages returns the new consumer.
    g.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![
            (p("/proj/layouts/main.tsx"), DepKind::Module),
            (p("/proj/components/header.tsx"), DepKind::Module),
            (p("/proj/components/new-widget.tsx"), DepKind::Module),
            (p("/proj/styles/global.css"), DepKind::Style),
        ],
    ));
    let d = g.dirty_pages(&p("/proj/components/new-widget.tsx"));
    assert_eq!(d.as_pages().unwrap(), vec![pid("/proj/pages/index.tsx")]);
}

#[test]
fn adding_a_brand_new_page_appears_in_pages_list() {
    let mut g = fixture();
    g.upsert(PageDeps::new(
        pid("/proj/pages/new.tsx"),
        vec![(p("/proj/layouts/main.tsx"), DepKind::Module)],
    ));
    assert_eq!(g.page_count(), 5);
    // And the shared layout now dirties four pages instead of three.
    let d = g.dirty_pages(&p("/proj/layouts/main.tsx"));
    assert_eq!(d.as_pages().unwrap().len(), 4);
}
