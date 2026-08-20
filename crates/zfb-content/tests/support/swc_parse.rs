use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast::EsVersion;
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

#[derive(Default)]
pub struct CompileOptions {
    filename: String,
}

impl CompileOptions {
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = filename.into();
        self
    }
}

pub struct Compiled {
    pub code: String,
}

pub struct SwcPipeline;

impl SwcPipeline {
    pub fn new() -> Self {
        Self
    }

    pub fn compile(&self, source: &str, options: &CompileOptions) -> Result<Compiled, String> {
        let cm: Lrc<SourceMap> = Default::default();
        let file = cm.new_source_file(
            FileName::Custom(options.filename.clone()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: true,
                ..Default::default()
            }),
            EsVersion::Es2022,
            StringInput::from(&*file),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        parser
            .parse_module()
            .map_err(|error| format!("{:?}", error.kind()))?;
        if let Some(error) = parser.take_errors().first() {
            return Err(format!("{:?}", error.kind()));
        }
        Ok(Compiled {
            code: source.to_string(),
        })
    }
}
