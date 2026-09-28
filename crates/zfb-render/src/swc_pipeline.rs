//! SWC TSX → JS pipeline.
//!
//! Responsibilities:
//! - Parse TypeScript + JSX into an AST (`swc_ecma_parser` w/ `tsx: true`).
//! - Apply the automatic JSX transform targeting `@takazudo/zfb/zudo-react`.
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

/// Compile-time options handed to the SWC pipeline.
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// Display name / path used in source maps and error messages.
    pub filename: String,
    /// Whether to dev-mode the JSX transform (preserves `__source` /
    /// `__self`). Off by default for SSR.
    pub development: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            filename: "<anonymous>.tsx".to_string(),
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
                    import_source: Some(Atom::from("@takazudo/zfb/zudo-react")),
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
    fn transforms_jsx_with_owned_runtime() {
        let src = "export default function Page(){ return <div>hello</div>; }\n";
        let out = SwcPipeline::new()
            .compile(src, &CompileOptions::default().with_filename("page.tsx"))
            .expect("compile ok");
        assert!(
            out.code.contains("@takazudo/zfb/zudo-react/jsx-runtime"),
            "expected owned JSX import, got: {}",
            out.code
        );
        assert!(!out.code.contains("<div>"));
    }

    #[test]
    fn development_transform_uses_owned_dev_runtime() {
        let src = "export default function Page(){ return <span/>; }\n";
        let out = SwcPipeline::new()
            .compile(
                src,
                &CompileOptions {
                    development: true,
                    ..CompileOptions::default()
                },
            )
            .expect("compile ok");
        assert!(out
            .code
            .contains("@takazudo/zfb/zudo-react/jsx-dev-runtime"));
    }
}
