//! Reproduction tests for bug #1284 (epic #1285), Wave-1 diagnosis #1286 —
//! CSS engine level (Level 1, pure logic, no V8).
//!
//! - **Symptom C** — a NEW Tailwind utility class authored inside a component
//!   under `src/**` is never emitted into `/assets/styles.css`. Root cause
//!   pinned here: `DEFAULT_CONTENT_ROOTS` (the `@source` scan roots Tailwind
//!   walks for utility classes) is `["pages","components","layouts","content"]`
//!   — it OMITS `src/`. So a class in `src/components/Foo.tsx` is outside every
//!   scanned `@source` glob and never reaches the generated stylesheet. (The
//!   second half of symptom C — the scan not re-running on a `.tsx` Module edit
//!   — is pinned in the orchestrator test `component_edit_triggers_css_rescan`.)
//!
//! - **Symptom B (engine half)** — `zfb-css` does NOT resolve local CSS
//!   `@import` file dependencies. `build_synthesised_entry_css` only prepends
//!   `@import "tailwindcss";` + `@source` directives and passes the user CSS
//!   through verbatim; a user `@import './local.css'` / `@import
//!   '@scope/design-system'` is left for the Tailwind CLI to resolve invisibly,
//!   so zfb never learns the real (canonicalised) dependency path to watch.
//!
//! The "current_bug_*" tests assert TODAY's behaviour (not ignored — they lock
//! the boundary). Fixed-behaviour tests are `#[ignore = "pending fix: #1284"]`.

use std::path::Path;

use zfb_css::engine::{default_source_directives, DEFAULT_CONTENT_ROOTS};

/// SYMPTOM C (current) — the scan roots omit `src/`, so a `src/**` component is
/// never scanned for utility classes. Documents present behaviour; not ignored.
#[test]
fn current_bug_src_root_is_not_a_scan_root() {
    assert!(
        !DEFAULT_CONTENT_ROOTS.contains(&"src"),
        "today src/ is NOT a Tailwind scan root, so classes authored under src/ are dropped"
    );
    let dirs = default_source_directives(Path::new("/proj"));
    assert!(
        !dirs.contains("/proj/src"),
        "the emitted @source directives do not cover /proj/src today"
    );
    // Sanity: the roots it DOES cover are present.
    assert!(dirs.contains("/proj/components"));
}

/// SYMPTOM C (fixed) — after the fix, `src/` is a scan root so a new utility
/// class authored in a `src/**` component is emitted. Ignored until fix lands.
#[test]
#[ignore = "pending fix: #1284"]
fn src_root_is_scanned_for_utility_classes() {
    assert!(
        DEFAULT_CONTENT_ROOTS.contains(&"src"),
        "fix adds src/ to the Tailwind scan roots"
    );
    let dirs = default_source_directives(Path::new("/proj"));
    assert!(
        dirs.contains("/proj/src"),
        "after the fix the emitted @source directives cover /proj/src"
    );
}

/// SYMPTOM B (engine half, fixed) — after #1288 the entry-CSS build (or a
/// sibling resolver) must surface the local/workspace `@import` targets so the
/// dev watcher can register their canonicalised real paths. Today there is no
/// API that returns the resolved `@import` dep set; this asserts the post-fix
/// contract and is ignored until it exists.
///
/// Marked ignored AND left as a contract placeholder: the exact API shape is
/// the #1288 author's call (per D2). Re-home/implement this assertion against
/// the real resolver entry point when it lands.
#[test]
#[ignore = "pending fix: #1284 — @import resolver API is owned by #1288 (D2)"]
fn local_css_imports_are_resolved_to_real_paths() {
    // Placeholder for the post-fix observable:
    //   given an entry CSS containing
    //     @import './tokens.css';
    //     @import '@scope/design-system';
    //   the resolver returns the canonicalised real file paths (following the
    //   workspace symlink) so the dev layer can watch them.
    //
    // Once #1288 adds the resolver, replace this with a real call + assertion.
    panic!("contract placeholder: implement against #1288's @import resolver (D2)");
}
