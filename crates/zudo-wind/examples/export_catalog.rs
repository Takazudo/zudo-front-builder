use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = crate_dir.join("catalog/zudo-wind-catalog.v1.json");
    fs::create_dir_all(output.parent().expect("catalog file has a parent"))?;
    fs::write(output, zudo_wind::catalog::export::export_json_pretty()?)?;
    Ok(())
}
