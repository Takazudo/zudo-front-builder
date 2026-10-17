//! Temporary phase-1 evidence postprocessor, outside the candidate graph.
//!
//! Reads one JSON request per stdin line and uses the existing production
//! HTML helpers or source-map decoder. Map coordinates are 1-based.
//! Build this example with `--no-default-features` to avoid a V8 dependency.

use std::io::{self, BufRead, Write};

use serde::Deserialize;
use serde_json::{json, Value};
use zfb_build::{
    inject_prod_head_assets, needs_html5_doctype, ProdHeadAssets, HTML5_DOCTYPE_PREFIX,
};
use zfb_render::sourcemap::decode_position;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Request {
    Html {
        html: String,
        css_url: Option<String>,
        #[serde(default)]
        island_module_urls: Vec<String>,
    },
    Map {
        map_json: String,
        line: usize,
        column: usize,
    },
}

fn execute(request: Request) -> Result<Value, String> {
    match request {
        Request::Html {
            html,
            css_url,
            island_module_urls,
        } => {
            let assets = ProdHeadAssets {
                css_url,
                island_module_urls,
            };
            let injected = inject_prod_head_assets(&html, &assets);
            let output = if needs_html5_doctype(&injected) {
                format!("{HTML5_DOCTYPE_PREFIX}{injected}")
            } else {
                injected.into_owned()
            };
            Ok(json!(output))
        }
        Request::Map {
            map_json,
            line,
            column,
        } => {
            if line == 0 || column == 0 {
                return Err("source-map line and column must be 1-based".to_owned());
            }
            let frame = decode_position(&map_json, line, column).ok_or_else(|| {
                "source map is invalid or does not cover the requested position".to_owned()
            })?;
            Ok(json!({
                "file": frame.file,
                "line": frame.line,
                "column": frame.col,
                "source_content": frame.source_content,
            }))
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        let result = serde_json::from_str::<Request>(&line)
            .map_err(|error| format!("invalid postprocessor request: {error}"))
            .and_then(execute);
        let receipt = match result {
            Ok(value) => json!({ "ok": true, "value": value }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        writeln!(
            stdout,
            "ZFB_POSTPROCESS_RECEIPT {}",
            serde_json::to_string(&receipt)?
        )?;
        stdout.flush()?;
    }
    Ok(())
}
