//! SWC TSX → JS pipeline.
//!
//! Responsibilities:
//! - Parse TypeScript + JSX into an AST (`swc_ecma_parser` w/ `tsx: true`).
//! - Apply the React JSX transform (`automatic` runtime; configurable
//!   `import_source` so Sub 4 can flip between `"preact"` and `"react"`).
//! - Strip TS type annotations.
//! - Emit ES module JS that the JS runtime can load.
//!
//! Source maps and accurate spans are preserved so the JS runtime can produce
//! source-location-accurate error messages downstream.

use swc_core::atoms::Atom;
use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{EsVersion, Program};
use swc_core::ecma::codegen::text_writer::JsWriter;
use swc_core::ecma::codegen::{Config as CodegenConfig, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::transforms::base::hygiene::hygiene;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::transforms::react::{react, Options as ReactOptions, Runtime};
use swc_core::ecma::transforms::typescript::strip;

use crate::error::{RenderError, Result};

/// Which JSX runtime SWC's `transform-react` should target.
///
/// In `Automatic` mode, this drives the synthetic `import { jsx } from "<x>/jsx-runtime"`
/// inserted by SWC. The actual implementation of those imports is provided by
/// the framework adapter (Sub 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JsxRuntime {
    /// `import_source = "preact"` (the default for zfb).
    #[default]
    Preact,
    /// `import_source = "react"`.
    React,
}

impl JsxRuntime {
    /// Stringly-typed `import_source` value handed to SWC.
    pub fn import_source(self) -> &'static str {
        match self {
            JsxRuntime::Preact => "preact",
            JsxRuntime::React => "react",
        }
    }
}

/// Compile-time options handed to the SWC pipeline.
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// Display name / path used in source maps and error messages.
    pub filename: String,
    /// Which JSX runtime to target.
    pub jsx_runtime: JsxRuntime,
    /// Whether to dev-mode the JSX transform (preserves `__source` /
    /// `__self`). Off by default for SSR.
    pub development: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            filename: "<anonymous>.tsx".to_string(),
            jsx_runtime: JsxRuntime::Preact,
            development: false,
        }
    }
}

impl CompileOptions {
    /// Set a filename for diagnostics / source maps.
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = filename.into();
        self
    }

    /// Pick the JSX runtime (Sub 4 flips this from the framework adapter).
    pub fn with_jsx_runtime(mut self, runtime: JsxRuntime) -> Self {
        self.jsx_runtime = runtime;
        self
    }
}

/// Output of the SWC pipeline: ES-module JavaScript ready to be loaded by the
/// JS runtime.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Display name / specifier the JS runtime will associate with this code.
    pub specifier: String,
    /// ES module JavaScript source.
    pub code: String,
}

/// SWC TSX → JS pipeline.
#[derive(Debug, Default)]
pub struct SwcPipeline;

impl SwcPipeline {
    /// Construct an empty pipeline. The pipeline holds no state today; this
    /// constructor exists so future versions can stash a parser cache here
    /// without breaking call sites.
    pub fn new() -> Self {
        Self
    }

    /// Compile a single TSX source string into ES module JavaScript.
    pub fn compile(&self, source: &str, opts: &CompileOptions) -> Result<CompiledModule> {
        let cm: Lrc<SourceMap> = Default::default();
        let comments = SingleThreadedComments::default();
        let fm = cm.new_source_file(
            FileName::Real(opts.filename.clone().into()).into(),
            source.to_string(),
        );

        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: true,
                decorators: false,
                dts: false,
                no_early_errors: false,
                disallow_ambiguous_jsx_like: false,
            }),
            EsVersion::Es2022,
            StringInput::from(&*fm),
            Some(&comments),
        );
        let mut parser = Parser::new_from(lexer);

        let module = parser
            .parse_module()
            .map_err(|e| RenderError::compile(&opts.filename, format!("parse failed: {e:?}")))?;

        // Run all transforms inside a fresh `Globals` scope so `Mark`s are
        // isolated and don't leak across compiles.
        let globals = Globals::new();
        let code = GLOBALS.set(&globals, || -> Result<String> {
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();

            // Compose the pass pipeline. SWC 64 unified transforms behind the
            // `Pass` trait — apply each pass against a `Program`, then unwrap
            // the `Module`.
            let mut program = Program::Module(module);

            program = program.apply(resolver(unresolved_mark, top_level_mark, true));
            program = program.apply(react::<SingleThreadedComments>(
                cm.clone(),
                Some(comments.clone()),
                ReactOptions {
                    runtime: Some(Runtime::Automatic),
                    import_source: Some(Atom::from(opts.jsx_runtime.import_source())),
                    development: Some(opts.development),
                    ..Default::default()
                },
                top_level_mark,
                unresolved_mark,
            ));
            program = program.apply(strip(unresolved_mark, top_level_mark));
            program = program.apply(hygiene());
            program = program.apply(fixer(Some(&comments)));

            let module = match program {
                Program::Module(m) => m,
                Program::Script(_) => {
                    return Err(RenderError::compile(
                        &opts.filename,
                        "expected ES module, got script",
                    ));
                }
            };

            let mut buf = Vec::new();
            {
                let writer = JsWriter::new(cm.clone(), "\n", &mut buf, None);
                let mut emitter = Emitter {
                    cfg: CodegenConfig::default().with_target(EsVersion::Es2022),
                    cm: cm.clone(),
                    comments: Some(&comments),
                    wr: writer,
                };
                emitter.emit_module(&module).map_err(|e| {
                    RenderError::compile(&opts.filename, format!("codegen failed: {e}"))
                })?;
            }

            String::from_utf8(buf)
                .map_err(|e| RenderError::compile(&opts.filename, format!("utf-8 error: {e}")))
        })?;

        Ok(CompiledModule {
            specifier: opts.filename.clone(),
            code,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_typescript_annotations() {
        let src = "export const greeting: string = \"hello\";\n";
        let out = SwcPipeline::new()
            .compile(src, &CompileOptions::default().with_filename("greet.ts"))
            .expect("compile ok");
        assert!(out.code.contains("greeting"));
        // Type annotation must be gone.
        assert!(!out.code.contains(": string"));
    }

    #[test]
    fn transforms_jsx_with_preact_runtime() {
        let src = "export default function Page(){ return <div>hello</div>; }\n";
        let out = SwcPipeline::new()
            .compile(
                src,
                &CompileOptions::default()
                    .with_filename("page.tsx")
                    .with_jsx_runtime(JsxRuntime::Preact),
            )
            .expect("compile ok");
        // Automatic runtime ⇒ synthetic import from `<source>/jsx-runtime`.
        assert!(
            out.code.contains("preact/jsx-runtime"),
            "expected preact/jsx-runtime import, got: {}",
            out.code
        );
        // No leftover JSX in output (must be desugared to function calls).
        assert!(!out.code.contains("<div>"));
    }

    #[test]
    fn transforms_jsx_with_react_runtime() {
        let src = "export default function P(){ return <span/>; }\n";
        let out = SwcPipeline::new()
            .compile(
                src,
                &CompileOptions::default()
                    .with_filename("p.tsx")
                    .with_jsx_runtime(JsxRuntime::React),
            )
            .expect("compile ok");
        assert!(
            out.code.contains("react/jsx-runtime"),
            "expected react/jsx-runtime import, got: {}",
            out.code
        );
    }
}
