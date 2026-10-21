//! Temporary phase-1 characterization of the unchanged outer config boundary.
//! This is the normal CLI crate's loader, not a candidate backend selector.

use std::io::{self, BufRead};

fn main() -> anyhow::Result<()> {
    #[cfg(any(feature = "embed_v8", feature = "direct-v8"))]
    zfb_render::initialize_v8().map_err(anyhow::Error::msg)?;
    run()
}

#[tokio::main(flavor = "current_thread")]
async fn run() -> anyhow::Result<()> {
    for line in io::stdin().lock().lines() {
        let input: serde_json::Value = serde_json::from_str(&line?)?;
        let source = input["source"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("source must be a string"))?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("zfb.config.ts");
        std::fs::write(&path, source)?;
        let result = zfb::config::load_from_file(&path).await;
        let receipt = match result {
            Ok(config) => serde_json::json!({
                "case": input["name"], "ok": true,
                "outDir": config.out_dir, "publicDir": config.public_dir,
                "host": config.host, "port": config.port,
            }),
            Err(error) => serde_json::json!({
                "case": input["name"], "ok": false,
                "error": format!("{error:#}").replace(
                    &directory.path().to_string_lossy().to_string(), "<fixture>"),
            }),
        };
        let mut receipt = receipt;
        receipt["engine"] = serde_json::json!(if cfg!(feature = "embed_v8") {
            "deno_core 0.399.0 / v8 147.4.0"
        } else if cfg!(feature = "direct-v8") {
            "direct v8 147.4.0"
        } else {
            "node subprocess"
        });
        println!("ZFB_OUTER_CONFIG_RECEIPT {receipt}");
    }
    Ok(())
}
