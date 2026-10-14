//! Temporary phase-1 characterization of the unchanged outer config boundary.
//! This is the normal CLI crate's loader, not a candidate backend selector.

use std::io::{self, BufRead};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
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
        println!("ZFB_OUTER_CONFIG_RECEIPT {receipt}");
    }
    Ok(())
}
