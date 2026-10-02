//! Issue #998 — client helpers do not become mountable-island entries.
//!
//! Two layers are covered:
//!
//! 1. **Boundary target discovery** (via public [`scan_islands`]) registers
//!    only concrete functions supplied to the SDK boundary. Unused client
//!    exports remain helpers. Opaque values demanded as targets fail loudly.
//!
//! 2. **Generated-source runtime guard** (`render_shared_bundle_entry_source`) — the shared bundle entry reject non-component values with a loud `console.warn`
//!    instead of a truthy-only check that would hand a bogus type to
//!    `h()` / `createElement()`.
//!
//! These live in a dedicated test file (NOT appended to `integration.rs`)
//! so a sibling topic adding tests there cannot conflict.

use std::path::PathBuf;

use zfb_islands::{render_shared_bundle_entry_source, scan_islands, InMemoryResolver, Island};

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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><DesktopSidebarToggle/></Island>; }
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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Widget/></Island>; }
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
fn opaque_call_expression_target_is_rejected() {
    // A call expression is not proof of the returned function's identity.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Memoized } from "../components/calls";
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Memoized/></Island>; }
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

    let error = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap_err();
    assert!(
        error.to_string().contains("unsupported initializer"),
        "{error}"
    );
    assert!(error.to_string().contains("named function"), "{error}");
}

#[test]
fn opaque_tagged_template_target_is_rejected() {
    // A tagged template may produce a function, but does not prove identity.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Box } from "../components/styled";
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Box/></Island>; }
            "#,
        )
        .with_file(
            root().join("components/styled.tsx"),
            "\"use client\";\nexport const Box = styled.div`color: red;`;\n",
        );

    let error = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap_err();
    assert!(
        error.to_string().contains("unsupported initializer"),
        "{error}"
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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Bar/></Island>; }
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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Foo/></Island>; }
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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Widget/></Island>; }
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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Widget/></Island>; }
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
fn source_reexport_is_registered_when_demanded() {
    // A concrete target can pass through a client barrel even when its
    // defining module has no directive.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget, Button } from "../components/barrel";
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <><Island><Widget/></Island><Island><Button/></Island></>; }
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
        "a cross-module source re-export registers only when bounded"
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
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Widget/></Island>; }
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

#[test]
fn parenthesized_literal_const_export_is_dropped() {
    // Documented spec decision (issue #998): `init_is_clearly_non_component`
    // peels a redundant wrapping paren (`Expr::Paren`) before classifying,
    // so `export const K = ("str");` is still recognised as a string literal
    // and dropped — the paren must not shield a non-component value from the
    // filter.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { Widget } from "../components/paren-literal";
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><Widget/></Island>; }
            "#,
        )
        .with_file(
            root().join("components/paren-literal.tsx"),
            r#""use client";
            export const K = ("str");
            export function Widget() { return null; }
            "#,
        );

    assert_eq!(
        component_names(&resolver, "components/paren-literal.tsx"),
        vec!["Widget".to_string()],
        "a parenthesized string-literal const must be peeled and dropped"
    );
}

#[test]
fn ts_cast_object_target_is_rejected() {
    // A cast cannot turn an object literal into a known function binding.
    let resolver = InMemoryResolver::new()
        .with_file(
            root().join("pages/home.tsx"),
            r#"import { AsCast } from "../components/ts-casts";
            import { Island } from "@takazudo/zfb";
            export default function Home() { return <Island><AsCast/></Island>; }
            "#,
        )
        .with_file(
            root().join("components/ts-casts.tsx"),
            r#""use client";
            export const AsCast = ({} as any);
            export const SatisfiesCast = ({} satisfies unknown);
            export const NonNullCast = ({})!;
            "#,
        );

    let error = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap_err();
    assert!(
        error.to_string().contains("unsupported initializer"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Generated-source runtime guard
// ---------------------------------------------------------------------------

#[test]
fn shared_bundle_entry_uses_component_shape_guard_not_truthy_only() {
    let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];

    let src = render_shared_bundle_entry_source(&islands, false);
    // The old truthy-only guard must be gone.
    assert!(
        !src.contains("if (!C) return;"),
        "truthy-only guard `if (!C) return;` must be replaced:\n{src}"
    );
    // Component-shape guard present.
    assert!(
        src.contains("typeof C !== \"function\""),
        "expected typeof-function check:\n{src}"
    );
    assert!(
        src.contains("name !== markerName"),
        "expected static identity check against the scanner marker:\n{src}"
    );
    // Loud, non-silent rejection naming the export + module.
    assert!(
        src.contains("throw new Error(") && src.contains("must be a function"),
        "expected a hard error on rejection:\n{src}"
    );
    // The module label is threaded through as the 4th register arg.
    assert!(
        src.contains("__zfb_register(__zfb_island_0, \"Counter\", \"Counter\", \"Counter.tsx\");"),
        "expected module label passed as the 4th __zfb_register arg:\n{src}"
    );
}
