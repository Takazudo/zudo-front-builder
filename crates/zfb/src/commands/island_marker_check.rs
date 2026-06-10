//! Issue #984 / #990: build-time warning when a rendered
//! `data-zfb-island="X"` / `data-zfb-island-skip-ssr="X"` marker has no
//! matching entry in the islands registry.
//!
//! ## Why this is non-fatal
//!
//! A missing registry entry turns the island into a dead-island at runtime
//! (the hydration manifests skips it), which is a silent UX failure rather
//! than a build-time crash.  The same non-fatal posture as the #122 / #822
//! scanner warnings is appropriate here: warn loudly, never block shipping.
//!
//! ## Why production only
//!
//! The dev bundle already surfaces this in the browser console via
//! `packages/zfb/src/runtime.ts:475-485` (`NODE_ENV=development` warn path).
//! This build pass closes the gap for production builds, where no runtime
//! warning fires.
//!
//! ## Known coverage gaps
//!
//! - **SSR-only routes** (`prerender = false`) are not rendered to disk by the
//!   SSG pass and therefore never reach this check.  An SSR route with an
//!   unregistered island will silently ship the dead-island to the runtime.
//! - **`.html`-source verbatim pages** (the `static_html` flag) bypass the
//!   renderer entirely; their HTML is copied as-is and is not in
//!   `post_processable_pages`, so this pass never sees them.
//!
//! ## Selector vs substring
//!
//! This pass uses `lol_html`'s CSS attribute selector (`[data-zfb-island]`,
//! `[data-zfb-island-skip-ssr]`) rather than a substring scan.  This mirrors
//! the approach taken in `crates/zfb-islands/src/hydration.rs:471-478` and
//! avoids false-positive matches inside `<pre>` / `<code>` / Markdown-rendered
//! code blocks that contain the literal attribute string.  See the hazard note
//! at `crates/zfb-islands/src/hydration.rs:239-243`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use lol_html::html_content::Element;
use lol_html::{ElementContentHandlers, RewriteStrSettings, Selector};

use crate::output;

/// Walk `pages` (the SSG post-processable HTML files) and emit one
/// `output::warn` per unknown marker name — a name that appears in a
/// `data-zfb-island` or `data-zfb-island-skip-ssr` attribute but is absent
/// from `registered_names`.
///
/// The check is non-fatal: no `Result` is returned.  The caller must not
/// treat a non-empty warning set as a build error.
///
/// Runs even when `registered_names` is empty (the "zero registered islands
/// but at least one rendered marker" scenario is exactly the #984 case).
pub fn check_island_markers(pages: &[PathBuf], registered_names: &BTreeSet<String>) {
    // Collect (marker_name → first path that contained it, count of pages).
    let mut unknown: BTreeMap<String, (PathBuf, usize)> = BTreeMap::new();

    for page in pages {
        if !is_html_path(page) {
            continue;
        }
        let bytes = match std::fs::read(page) {
            Ok(b) => b,
            Err(_) => continue, // best-effort; read failure is non-fatal here
        };
        // Fast pre-filter: skip pages that do not mention the attribute at all.
        // `contains` on bytes avoids a UTF-8 conversion for the cold path.
        if !bytes
            .windows(b"data-zfb-island".len())
            .any(|w| w == b"data-zfb-island")
        {
            continue;
        }
        let html = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(_) => continue, // binary-ish file with .html extension — skip
        };
        let names = collect_marker_names_in_page(html);
        for name in names {
            if !registered_names.contains(&name) {
                unknown
                    .entry(name)
                    .and_modify(|(_, count)| *count += 1)
                    .or_insert_with(|| (page.clone(), 1));
            }
        }
    }

    for (name, (first_page, extra_count)) in unknown {
        let location = if extra_count > 1 {
            format!(
                "{} (+{} more page{})",
                first_page.display(),
                extra_count - 1,
                if extra_count - 1 == 1 { "" } else { "s" },
            )
        } else {
            first_page.display().to_string()
        };

        if name == "Anonymous" {
            // "Anonymous" is the captureComponentName fallback
            // (packages/zfb/src/island.ts:71) — it means the SSR side could
            // not recover a component name at render time.  The usual
            // export-shape hint would mislead here because the problem is not
            // an unrecognised export shape but a missing displayName /
            // function identifier on the component.
            output::warn(format!(
                "island marker \"Anonymous\" found in {location} has no matching registry entry. \
                 This usually means the island component is an anonymous function or arrow \
                 expression with no `displayName`. Add a named function or set \
                 `Component.displayName` so the SSR side can recover a stable marker name.",
            ));
        } else {
            output::warn(format!(
                "island marker \"{name}\" found in {location} has no matching registry entry \
                 and will not hydrate. \
                 Ensure the component is exported as a named binding (e.g. \
                 `export function {name}()` or `export const {name} = …`) with \
                 the `\"use client\"` directive and is reachable from a file in pages/. \
                 If `{name}` comes from a `displayName` assignment, verify the \
                 scanner-visible export identifier matches.",
            ));
        }
    }
}

/// Collect every non-empty `data-zfb-island` and `data-zfb-island-skip-ssr`
/// attribute value in `html` using a `lol_html` element-selector pass.
///
/// Returns a deduplicated `BTreeSet` of marker names found in the page.
///
/// Uses the attribute selector (`[data-zfb-island]` / `[data-zfb-island-skip-ssr]`)
/// rather than a substring scan — see module-level note for rationale.
fn collect_marker_names_in_page(html: &str) -> BTreeSet<String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let names: Rc<RefCell<BTreeSet<String>>> = Rc::new(RefCell::new(BTreeSet::new()));

    // Selector for `data-zfb-island` attribute (non-empty only — empty value
    // is the skeleton placeholder that the runtime skips; runtime.ts:362-367
    // skips hydrating elements whose data-zfb-island attribute is empty).
    let island_selector: Selector = "[data-zfb-island]"
        .parse()
        .expect("static selector is valid");
    // Selector for the SSR-skip variant.
    let skip_ssr_selector: Selector = "[data-zfb-island-skip-ssr]"
        .parse()
        .expect("static selector is valid");

    let names_a = Rc::clone(&names);
    let names_b = Rc::clone(&names);

    let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
        element_content_handlers: vec![
            (
                Cow::Borrowed(&island_selector),
                ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                    if let Some(val) = el.get_attribute("data-zfb-island") {
                        if !val.is_empty() {
                            names_a.borrow_mut().insert(val);
                        }
                    }
                    Ok(())
                }),
            ),
            (
                Cow::Borrowed(&skip_ssr_selector),
                ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                    if let Some(val) = el.get_attribute("data-zfb-island-skip-ssr") {
                        if !val.is_empty() {
                            names_b.borrow_mut().insert(val);
                        }
                    }
                    Ok(())
                }),
            ),
        ],
        ..RewriteStrSettings::new()
    };

    // We only need to scan (read-only) — discard the rewritten output.
    let _ = lol_html::rewrite_str(html, settings);
    Rc::try_unwrap(names).unwrap_or_default().into_inner()
}

fn is_html_path(p: &std::path::Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()),
        Some("html") | Some("htm")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_html(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    // --- collect_marker_names_in_page ----------------------------------------

    #[test]
    fn collect_finds_data_zfb_island_attribute() {
        let html = r#"<div data-zfb-island="Counter"></div>"#;
        let names = collect_marker_names_in_page(html);
        assert!(names.contains("Counter"), "got: {names:?}");
    }

    #[test]
    fn collect_finds_data_zfb_island_skip_ssr_attribute() {
        let html = r#"<div data-zfb-island-skip-ssr="AiChat"></div>"#;
        let names = collect_marker_names_in_page(html);
        assert!(names.contains("AiChat"), "got: {names:?}");
    }

    #[test]
    fn collect_skips_empty_value() {
        // Empty data-zfb-island is the unfilled skeleton; runtime skips it too.
        let html = r#"<div data-zfb-island=""></div>"#;
        let names = collect_marker_names_in_page(html);
        assert!(names.is_empty(), "expected empty set; got: {names:?}");
    }

    #[test]
    fn collect_deduplicates_same_name_across_elements() {
        let html = r#"
            <div data-zfb-island="Counter"></div>
            <div data-zfb-island="Counter"></div>
        "#;
        let names = collect_marker_names_in_page(html);
        assert_eq!(names.len(), 1, "expected dedup; got: {names:?}");
        assert!(names.contains("Counter"));
    }

    #[test]
    fn collect_finds_multiple_distinct_markers() {
        let html = r#"
            <div data-zfb-island="Counter"></div>
            <div data-zfb-island="Toggle"></div>
            <div data-zfb-island-skip-ssr="AiModal"></div>
        "#;
        let names = collect_marker_names_in_page(html);
        assert!(names.contains("Counter"), "got: {names:?}");
        assert!(names.contains("Toggle"), "got: {names:?}");
        assert!(names.contains("AiModal"), "got: {names:?}");
    }

    /// Code fence / pre-code literals must NOT trigger — the selector-based
    /// approach means only real DOM elements match, not text content inside
    /// `<pre>` or Markdown-rendered code blocks.
    #[test]
    fn collect_does_not_false_positive_on_code_fence_literal() {
        let html = r#"
            <!doctype html><html><body>
            <pre><code>data-zfb-island="FakeName"</code></pre>
            </body></html>
        "#;
        let names = collect_marker_names_in_page(html);
        assert!(
            !names.contains("FakeName"),
            "code-fence text must not match; got: {names:?}"
        );
    }

    // --- check_island_markers (file I/O path) --------------------------------

    /// A page with a rendered marker absent from the registry produces one warn.
    #[test]
    fn check_warns_for_unknown_marker() {
        let tmp = tempdir().unwrap();
        let page = write_html(
            tmp.path(),
            "index.html",
            r#"<!doctype html><html><body>
                <div data-zfb-island="MissingWidget" data-props="{}"></div>
            </body></html>"#,
        );
        // The registry is empty — MissingWidget was never scanned.
        let registered = BTreeSet::new();
        // check_island_markers is non-fatal; capture by running it and
        // verifying collect found the name (the warn itself is side-effectful
        // and not directly assertable, but collect is the path under test).
        let found = collect_marker_names_in_page(&fs::read_to_string(&page).unwrap());
        assert!(found.contains("MissingWidget"), "got: {found:?}");
        // Calling the top-level function must not panic.
        check_island_markers(&[page], &registered);
    }

    /// A fully registered site should not trigger any warning (no panic
    /// either, but that's trivially checked by the test completing).
    #[test]
    fn check_clean_site_no_warnings() {
        let tmp = tempdir().unwrap();
        let page = write_html(
            tmp.path(),
            "index.html",
            r#"<!doctype html><html><body>
                <div data-zfb-island="Counter" data-props="{}"></div>
            </body></html>"#,
        );
        let mut registered = BTreeSet::new();
        registered.insert("Counter".to_string());
        check_island_markers(&[page], &registered);
    }

    /// An empty attribute value (skeleton placeholder) must not warn.
    #[test]
    fn check_skips_empty_attribute_value() {
        let tmp = tempdir().unwrap();
        let page = write_html(
            tmp.path(),
            "index.html",
            r#"<!doctype html><html><body>
                <div data-zfb-island=""></div>
            </body></html>"#,
        );
        let registered = BTreeSet::new();
        // collect must return empty — meaning check_island_markers emits no warn.
        let found = collect_marker_names_in_page(&fs::read_to_string(&page).unwrap());
        assert!(
            found.is_empty(),
            "empty value must not be collected; got: {found:?}"
        );
        check_island_markers(&[page], &registered);
    }

    /// Non-HTML files (XML, JSON, …) are skipped by `is_html_path`.
    #[test]
    fn check_skips_non_html_files() {
        let tmp = tempdir().unwrap();
        let xml = tmp.path().join("feed.xml");
        fs::write(&xml, r#"<item data-zfb-island="Ghost"></item>"#).unwrap();
        let registered = BTreeSet::new();
        // No panic; the XML file should be ignored (is_html_path returns false).
        check_island_markers(&[xml], &registered);
    }

    /// The check runs even when the registry is completely empty (zero registered
    /// islands + rendered marker = the #984 scenario).
    #[test]
    fn check_runs_with_empty_registry() {
        let tmp = tempdir().unwrap();
        let page = write_html(
            tmp.path(),
            "about.html",
            r#"<!doctype html><html><body>
                <div data-zfb-island="OrphanIsland" data-props="{}"></div>
            </body></html>"#,
        );
        // No islands registered at all.
        let registered = BTreeSet::new();
        // Must not panic; the warning is the intended side-effect.
        let found = collect_marker_names_in_page(&fs::read_to_string(&page).unwrap());
        assert!(found.contains("OrphanIsland"), "got: {found:?}");
        check_island_markers(&[page], &registered);
    }
}
