//! JSON-lines runner. No CLI/default integration and no JavaScript subprocess.
#[cfg(feature = "direct-v8")]
fn main() {
    use std::io::{BufRead, Write};
    use zfb_js_host::native::{evaluate_config_bundle, ENGINE_ID};

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let result = match line {
            Ok(line) => match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(input) if input["kind"] == "config" && input["source"].is_string() => {
                    match evaluate_config_bundle(input["source"].as_str().unwrap_or_default()) {
                        Ok(value) => {
                            serde_json::json!({"engine": ENGINE_ID, "ok": true, "value": value})
                        }
                        Err(error) => {
                            serde_json::json!({"engine": ENGINE_ID, "ok": false, "category": error.category(), "error": error.to_string()})
                        }
                    }
                }
                Ok(_) => {
                    serde_json::json!({"engine": ENGINE_ID, "ok": false, "category": "input", "error": "expected {kind:config,source:string}"})
                }
                Err(error) => {
                    serde_json::json!({"engine": ENGINE_ID, "ok": false, "category": "input", "error": error.to_string()})
                }
            },
            Err(error) => {
                let _ = writeln!(std::io::stderr(), "input read failed: {error}");
                std::process::exit(1);
            }
        };
        if writeln!(stdout, "ZFB_HOST_RECEIPT {result}")
            .and_then(|_| stdout.flush())
            .is_err()
        {
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "direct-v8"))]
fn main() {
    eprintln!("direct_v8 requires the explicit direct-v8 Cargo feature");
    std::process::exit(2);
}
