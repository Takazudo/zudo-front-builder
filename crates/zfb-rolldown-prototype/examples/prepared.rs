//! Minimal native capability/reproduction runner: the single argument is a
//! JSON-serialized typed `BundleRequest`. An empty `cwd` means the process cwd.
#[cfg(feature = "native")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    let mut args = std::env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        anyhow::bail!("usage: prepared <bundle-request.json>");
    };
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", std::path::Path::new(&path).display()))?;
    let request: zfb_rolldown_prototype::BundleRequest =
        serde_json::from_str(&text).context("parse typed bundle request")?;
    zfb_rolldown_prototype::bundle(&request)?;
    Ok(())
}
#[cfg(not(feature = "native"))]
fn main() {
    panic!("enable native feature");
}
