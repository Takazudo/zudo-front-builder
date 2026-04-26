//! Tiny smoke test to validate the rquickjs API path the bench uses.
use zfb_runtime_spike::host::{RenderHost, RenderInput};

fn main() -> anyhow::Result<()> {
    eprintln!("smoke: starting");
    let mut host = zfb_runtime_spike::hosts::quickjs::QuickJsHost::new()?;
    eprintln!("smoke: host ready");
    let src = "function renderHTML() { return '<p>hello</p>'; }";
    let out = host.render_module(RenderInput { name: "smoke", source: src })?;
    println!("RENDER: {}", out.html);
    Ok(())
}
