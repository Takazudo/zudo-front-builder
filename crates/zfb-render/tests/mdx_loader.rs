//! Integration tests for MDX-specifier handling in `ModuleLoader`.
//!
//! Verifies that when a specifier ends in `.mdx` or starts with `mdx://`,
//! `ModuleLoader::load_source` (and `load_file`) routes the source string
//! through `zfb_content::mdx_jsx_emit::mdx_to_jsx_module` before the SWC
//! TSX pass — and that compile errors from `zfb-content` surface as
//! `RenderError::Compile` carrying the original specifier.

use zfb_render::loader::ModuleLoader;
use zfb_render::swc_pipeline::JsxRuntime;
use zfb_render::RenderError;

// ---------------------------------------------------------------------------
// Happy path: <Note>foo</Note> in MDX compiles to a module with a default
// export.
// ---------------------------------------------------------------------------

/// MDX containing a PascalCase JSX element (`<Note>`) compiles end-to-end:
/// the loader runs MDX→JSX, hands the JSX to SWC, and the resulting JS
/// contains a default export. The `Note` component is intentionally NOT
/// wired up at compile time — the emitter generates a runtime check that
/// only fails when the component is invoked, not when the module loads.
#[test]
fn mdx_specifier_compiles_with_components_referenced() {
    let mut loader = ModuleLoader::new(JsxRuntime::Preact);

    let src = "# Title\n\n<Note>foo</Note>\n";

    let compiled = loader
        .load_source("post.mdx", src)
        .expect("MDX should compile through the loader");

    // The emitter produces `export default function MDXContent(...)` which
    // SWC lowers to one of these export forms depending on JSX runtime
    // settings. We assert both shapes so the test stays robust if SWC
    // changes its default-export codegen.
    let js = &compiled.code;
    assert!(
        js.contains("export default") || js.contains("export { ") && js.contains(" as default"),
        "expected a default export in compiled MDX, got:\n{js}"
    );

    // The `Note` identifier must survive the compile: the emitter creates
    // a `const Note = _components.Note ?? components.Note;` preamble and a
    // `throw new Error(...)` guard, both of which mention "Note".
    assert!(
        js.contains("Note"),
        "expected `Note` reference to survive into compiled JS, got:\n{js}"
    );
}

/// Same as above but using the `mdx://` URL form that
/// `compile_mdx_to_jsx_module` produces. The loader must recognise this
/// shape too — Sub 4 will hand it specifiers like
/// `mdx://blog/hello#abcd1234`.
#[test]
fn mdx_url_specifier_is_also_recognised() {
    let mut loader = ModuleLoader::new(JsxRuntime::Preact);

    let src = "<Note>hello</Note>\n";
    let specifier = "mdx://blog/hello#deadbeef";

    let compiled = loader
        .load_source(specifier, src)
        .expect("mdx:// specifier should route through the MDX emitter");

    let js = &compiled.code;
    assert!(
        js.contains("export default") || js.contains(" as default"),
        "expected default export, got:\n{js}"
    );
}

// ---------------------------------------------------------------------------
// Sad path: malformed MDX surfaces as RenderError::Compile carrying the
// original specifier.
// ---------------------------------------------------------------------------

/// Malformed MDX (an unterminated JSX tag) must produce a
/// `RenderError::Compile` whose `file` field equals the original specifier
/// — not the synthetic `<anonymous>.mdx` placeholder used by the emitter
/// when no filename is supplied.
#[test]
fn malformed_mdx_yields_compile_error_with_specifier_and_message() {
    let mut loader = ModuleLoader::new(JsxRuntime::Preact);

    // Unterminated JSX element — markdown-rs's MDX parser rejects this with
    // a positioned error message.
    let src = "<Note>oops\n";
    let specifier = "broken.mdx";

    let err = loader
        .load_source(specifier, src)
        .expect_err("malformed MDX must fail");

    match err {
        RenderError::Compile { file, message } => {
            assert_eq!(
                file, specifier,
                "compile error must carry the original specifier as `file`",
            );
            // markdown-rs error messages are non-empty and mention what it
            // expected. We don't pin the exact phrasing, only that the
            // message is informative (non-empty, more than a few chars).
            assert!(
                message.len() > 8,
                "compile error message should be informative, got: {message:?}",
            );
        }
        other => panic!("expected RenderError::Compile, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Regression: non-MDX specifiers still go straight to SWC.
// ---------------------------------------------------------------------------

/// A `.tsx` specifier must NOT be re-routed through the MDX emitter — it
/// should compile via the normal SWC path. This pins the loader's
/// dispatch so a future refactor doesn't accidentally widen the MDX
/// gate.
#[test]
fn tsx_specifier_skips_mdx_pipeline() {
    let mut loader = ModuleLoader::new(JsxRuntime::Preact);

    // Plain TSX with a type annotation — would be a parse error if the
    // MDX emitter were applied (MDX cannot parse `: number = 1`).
    let compiled = loader
        .load_source("page.tsx", "export const x: number = 1;\n")
        .expect("TSX should still compile straight through SWC");

    let js = &compiled.code;
    assert!(
        js.contains("export") && js.contains("x"),
        "expected `export const x` to survive, got:\n{js}"
    );
}
