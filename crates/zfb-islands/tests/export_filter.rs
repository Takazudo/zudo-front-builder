//! Issue #998 — only component-valued exports of `"use client"` modules
//! become mountable-island registry entries.
//!
//! Two layers are covered:
//!
//! 1. **Build-time AST filter** (`scanner::exported_island_records`, via the
//!    public [`scan_islands`] API) — clearly-non-component exports (string /
//!    number / object / array literals, untagged template strings,
//!    `export * as ns`, and local re-exports resolving to such values) are
//!    dropped; functions, classes, call expressions (`memo`/`forwardRef`/
//!    `lazy`/`styled`/…), tagged templates, and anything ambiguous are kept.
//!
//! 2. **Generated-source runtime guard** (`render_island_entry_source` /
//!    `render_shared_bundle_entry_source`) — the per-island and shared
//!    bundle entries reject non-component values with a loud `console.warn`
//!    instead of a truthy-only check that would hand a bogus type to
//!    `h()` / `createElement()`.
//!
//! These live in a dedicated test file (NOT appended to `integration.rs`)
//! so a sibling topic adding tests there cannot conflict.

use std::path::PathBuf;

use zfb_islands::{
    render_island_entry_source, render_shared_bundle_entry_source, scan_islands, FrameworkKind,
    InMemoryResolver, Island,
};

fn root() -> PathBuf {
    PathBuf::from("/proj")
}

/// Scan from a single page and return `(component_name, marker_name)` pairs
/// for the given source file, sorted for stable assertions.
fn records_for(resolver: &InMemoryResolver, source_rel: &str) -> Vec<(String, String)> {
    let islands = scan_islands(&[root().join("pages/home.tsx")], resolver).unwrap();
    let source = root().join(source_rel);
    let mut out: Vec<(String, String)> = islands
        .iter()
        .filter(|i| i.source_path == source)
        .map(|i| (i.component_name.clone(), i.marker_name.clone()))
        .collect();
    out.sort();
    out
}

fn component_names(resolver: &InMemoryResolver, source_rel: &str) -> Vec<String> {
    records_for(resolver, source_rel)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

// ---------------------------------------------------------------------------
// Build-time AST filter
// ---------------------------------------------------------------------------

#[test]
fn string_const_export_alongside_default_component_is_dropped() {
    // The exact repro from issue #998: a `"use client"` component module
    // that also exports a string constant (a localStorage key shared with
    // SSR code). Only the default component must be registered; the string
    // constant must NOT become a mountable island.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import DesktopSidebarToggle from "../components/desktop-sidebar-toggle";
            export default function Home() { return <DesktopSidebarToggle/>; }
            "#,
        )
        .with_file(
            root().join("components/desktop-sidebar-toggle.tsx"),
            r#""use client";
            export const SIDEBAR_STORAGE_KEY = 'zudo-doc-sidebar-visible';
            export default function DesktopSidebarToggle() { return null; }
            "#,
        );

    let records = records_for(&resolver, "components/desktop-sidebar-toggle.tsx");
    assert_eq!(
        records,
        vec![("default".to_string(), "DesktopSidebarToggle".to_string())],
        "only the default component must be registered; SIDEBAR_STORAGE_KEY \
         (a string constant) must be dropped"
    );
}

#[test]
fn number_object_and_array_literal_const_exports_are_dropped() {
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget } from "../components/literals";
            export default function Home() { return <Widget/>; }
            "#,
        )
        .with_file(
            root().join("components/literals.tsx"),
            r#""use client";
            export const COUNT = 42;
            export const ENABLED = true;
            export const NOTHING = null;
            export const CONFIG = { a: 1 };
            export const LIST = [1, 2, 3];
            export const LABEL = `static string`;
            export function Widget() { return null; }
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/literals.tsx"),
        vec!["Widget".to_string()],
        "primitive / object / array / template-string constants must all be dropped"
    );
}

#[test]
fn call_expression_exports_are_retained() {
    // memo(...) / forwardRef(...) / lazy(...) / styled(...) /
    // connect(...)(...) — ANY call expression is kept (no callee whitelist).
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Memoized } from "../components/calls";
            export default function Home() { return <Memoized/>; }
            "#,
        )
        .with_file(
            root().join("components/calls.tsx"),
            r#""use client";
            export const Memoized = memo(() => null);
            export const Forwarded = forwardRef(function Inner() { return null; });
            export const Lazied = lazy(() => import("./x"));
            export const Connected = connect(mapState)(function Base() { return null; });
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/calls.tsx"),
        vec![
            "Connected".to_string(),
            "Forwarded".to_string(),
            "Lazied".to_string(),
            "Memoized".to_string(),
        ],
        "every call-expression export must be retained"
    );
}

#[test]
fn tagged_template_export_is_retained() {
    // `styled.div\`...\`` parses as a tagged template (Expr::TaggedTpl),
    // distinct from an untagged template string — it must be kept.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Box } from "../components/styled";
            export default function Home() { return <Box/>; }
            "#,
        )
        .with_file(
            root().join("components/styled.tsx"),
            "\"use client\";\nexport const Box = styled.div`color: red;`;\n",
        );

    assert_eq!(
        component_names(&resolver, "components/styled.tsx"),
        vec!["Box".to_string()],
        "tagged-template export must be retained"
    );
}

#[test]
fn local_alias_export_of_arrow_const_is_retained() {
    // `const Foo = () => null; export { Foo as Bar }` — the local binding
    // resolves to a component shape and must be kept under the alias.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Bar } from "../components/alias";
            export default function Home() { return <Bar/>; }
            "#,
        )
        .with_file(
            root().join("components/alias.tsx"),
            r#""use client";
            const Foo = () => null;
            export { Foo as Bar };
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/alias.tsx"),
        vec!["Bar".to_string()],
        "local alias of an arrow-function const must be retained"
    );
}

#[test]
fn local_alias_export_as_default_of_function_is_retained() {
    // `function Foo() {}; export { Foo as default }` — the compiled
    // tsup/esbuild shape for `export default function Foo()`. Must be kept.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import Foo from "../components/default-alias";
            export default function Home() { return <Foo/>; }
            "#,
        )
        .with_file(
            root().join("components/default-alias.tsx"),
            r#""use client";
            function Foo() { return null; }
            export { Foo as default };
            "#,
        );

    assert_eq!(
        records_for(&resolver, "components/default-alias.tsx"),
        vec![("default".to_string(), "Foo".to_string())],
        "function re-exported as default must be retained with its local marker"
    );
}

#[test]
fn local_non_component_reexport_is_dropped() {
    // The re-export mirror of the #998 repro:
    // `const KEY = 'x'; export { KEY }` resolves to a string constant and
    // must be dropped, while a sibling component re-export is retained.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget } from "../components/reexport";
            export default function Home() { return <Widget/>; }
            "#,
        )
        .with_file(
            root().join("components/reexport.tsx"),
            r#""use client";
            const SIDEBAR_STORAGE_KEY = 'zudo-doc-sidebar-visible';
            const Widget = () => null;
            export { SIDEBAR_STORAGE_KEY, Widget };
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/reexport.tsx"),
        vec!["Widget".to_string()],
        "a local re-export resolving to a string constant must be dropped"
    );
}

#[test]
fn namespace_reexport_is_dropped() {
    // `export * as ns from "…"` binds a module-namespace object — never a
    // mountable component. It must be dropped while a sibling component is
    // retained.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget } from "../components/widget";
            export default function Home() { return <Widget/>; }
            "#,
        )
        .with_file(
            root().join("components/widget.tsx"),
            r#""use client";
            export * as utils from "./utils";
            export function Widget() { return null; }
            "#,
        )
        .with_file(
            root().join("components/utils.tsx"),
            r#"export const helper = 1;
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/widget.tsx"),
        vec!["Widget".to_string()],
        "`export * as utils` namespace re-export must be dropped"
    );
}

#[test]
fn source_reexport_is_kept_permissively() {
    // `export { Button } from "./button"` — resolving the other module is
    // out of scope, so the re-export is kept permissively (compiled package
    // barrels use this shape).
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget } from "../components/barrel";
            export default function Home() { return <Widget/>; }
            "#,
        )
        .with_file(
            root().join("components/barrel.tsx"),
            r#""use client";
            export { Button } from "./button";
            export function Widget() { return null; }
            "#,
        )
        .with_file(
            root().join("components/button.tsx"),
            r#"export function Button() { return null; }
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/barrel.tsx"),
        vec!["Button".to_string(), "Widget".to_string()],
        "a cross-module source re-export must be kept permissively"
    );
}

#[test]
fn default_string_literal_export_is_dropped() {
    // `export default 'foo'` is a clearly-non-component default; the module
    // then yields no islands at all (no near-miss assertion here, just that
    // the bogus default is not registered).
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget } from "../components/default-string";
            export default function Home() { return <Widget/>; }
            "#,
        )
        .with_file(
            root().join("components/default-string.tsx"),
            r#""use client";
            export function Widget() { return null; }
            export default 'just-a-string';
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/default-string.tsx"),
        vec!["Widget".to_string()],
        "`export default 'string'` must be dropped"
    );
}

// ---------------------------------------------------------------------------
// Generated-source runtime guard
// ---------------------------------------------------------------------------

#[test]
fn shared_bundle_entry_uses_component_shape_guard_not_truthy_only() {
    let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];

    for framework in [FrameworkKind::Preact, FrameworkKind::React] {
        let src = render_shared_bundle_entry_source(framework, &islands, false);
        // The old truthy-only guard must be gone.
        assert!(
            !src.contains("if (!C) return;"),
            "truthy-only guard `if (!C) return;` must be replaced ({framework:?}):\n{src}"
        );
        // Component-shape guard present.
        assert!(
            src.contains("typeof C === \"function\""),
            "expected typeof-function check ({framework:?}):\n{src}"
        );
        assert!(
            src.contains("C.$$typeof"),
            "expected $$typeof object check for memo/forwardRef ({framework:?}):\n{src}"
        );
        // Loud, non-silent rejection naming the export + module.
        assert!(
            src.contains("console.warn(") && src.contains("is not a component"),
            "expected a loud console.warn on rejection ({framework:?}):\n{src}"
        );
        // The module label is threaded through as the 4th register arg.
        assert!(
            src.contains(
                "__zfb_register(__zfb_island_0, \"Counter\", \"Counter\", \"/abs/components/Counter.tsx\");"
            ),
            "expected module label passed as the 4th __zfb_register arg ({framework:?}):\n{src}"
        );
    }
}

#[test]
fn per_island_entry_guards_mount_with_component_shape_check() {
    let island = Island::new("Counter", "/abs/components/Counter.tsx");

    for framework in [FrameworkKind::Preact, FrameworkKind::React] {
        let src = render_island_entry_source(framework, &island);
        assert!(
            src.contains("typeof Component === \"function\""),
            "expected typeof-function guard ({framework:?}):\n{src}"
        );
        assert!(
            src.contains("$$typeof"),
            "expected $$typeof object check ({framework:?}):\n{src}"
        );
        assert!(
            src.contains("if (!__zfb_ok) return;"),
            "mount must bail out when the export is not a component ({framework:?}):\n{src}"
        );
        assert!(
            src.contains("console.warn(")
                && src.contains("is not a component")
                && src.contains("/abs/components/Counter.tsx"),
            "expected a loud console.warn naming the module ({framework:?}):\n{src}"
        );
    }
}
