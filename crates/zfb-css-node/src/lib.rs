use std::path::PathBuf;

use napi::{bindgen_prelude::AsyncTask, Env, JsString, Task};
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{json, Value};
use zfb::commands::css::{absolute_path, compile_css, CssCompileOptions};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    input: PathBuf,
    cwd: PathBuf,
    project_root: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    config: Option<Value>,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    no_auto_source: bool,
    code_highlight_mode: Option<zfb::config::CodeHighlightMode>,
    #[serde(default)]
    no_default_highlight_styles: bool,
    output: Option<PathBuf>,
}

fn diagnostic(value: &zfb_css::CssDiagnostic) -> Value {
    json!({
        "code": value.code,
        "severity": match value.severity {
            zfb_css::CssDiagnosticSeverity::Warning => "warning",
            zfb_css::CssDiagnosticSeverity::Error => "error",
        },
        "message": value.message,
        "candidate": value.candidate,
        "origin": {
            "sourceId": value.origin.source_id,
            "path": value.origin.path,
            "line": value.origin.line,
            "column": value.origin.column,
            "byteOffset": value.origin.byte_offset,
            "label": value.origin.label,
        },
    })
}

fn compile(request: &str) -> anyhow::Result<Value> {
    let request: Request = serde_json::from_str(request)?;
    let root = absolute_path(
        &request.cwd,
        request.project_root.as_deref().unwrap_or(&request.cwd),
    );
    let config_dir = request
        .config_dir
        .as_deref()
        .map(|path| absolute_path(&request.cwd, path))
        .unwrap_or_else(|| root.clone());
    let (config, mut warnings) =
        zfb::config::prepare_css_config(request.config.unwrap_or_else(|| json!({})), &config_dir)?;
    let result = compile_css(CssCompileOptions {
        input: request.input,
        cwd: request.cwd,
        project_root: root,
        config,
        sources: request.sources,
        no_auto_source: request.no_auto_source,
        code_highlight_mode: request.code_highlight_mode,
        no_default_highlight_styles: request.no_default_highlight_styles,
        output: request.output,
    })?;
    warnings.extend(result.warnings);
    Ok(
        json!({"css": result.css, "diagnostics": result.diagnostics.iter().map(diagnostic).collect::<Vec<_>>(), "warnings": warnings}),
    )
}

pub struct CompileTask(String);

impl Task for CompileTask {
    type Output = String;
    type JsValue = JsString;

    fn compute(&mut self) -> napi::Result<String> {
        let value = match compile(&self.0) {
            Ok(result) => json!({"result": result}),
            Err(error) => {
                let diagnostics = error
                    .downcast_ref::<zfb_css::WindDiagnosticsError>()
                    .map(|error| error.diagnostics.iter().map(diagnostic).collect::<Vec<_>>())
                    .unwrap_or_default();
                json!({"error": {"message": format!("{error:#}"), "diagnostics": diagnostics}})
            }
        };
        Ok(value.to_string())
    }

    fn resolve(&mut self, env: Env, output: String) -> napi::Result<JsString> {
        env.create_string(&output)
    }
}

#[napi]
pub fn compile_css_native(request: String) -> AsyncTask<CompileTask> {
    AsyncTask::new(CompileTask(request))
}
