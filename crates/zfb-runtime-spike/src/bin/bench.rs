//! Bench binary. Runs a representative subset of fixtures through one or more
//! `RenderHost` implementations and writes a JSON report.
//!
//! Usage:
//!   cargo run --release -p zfb-runtime-spike --bin zfb-spike-bench
//!
//! Set `ZFB_SPIKE_ITERS=N` to control warm-loop iteration count (default 50).
//!
//! Which hosts run is gated by the cargo features the binary was built with:
//!   --features quickjs   (default)
//!   --features deno
//!   --features ssr-rs
//!   --features all-hosts
//!
//! The bench evaluates the pre-shaped ESM modules in
//! `target/spike-fixtures/bench-js/`. Run `zfb-spike-gen-fixtures` first if
//! that directory does not exist.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use zfb_runtime_spike::host::{RenderHost, RenderInput};
use zfb_runtime_spike::metrics::{
    current_rss_bytes, fmt_bytes, fmt_us, HostMetrics, MetricSample,
};

const FIXTURES: &[(&str, &str)] = &[
    ("static",       "bench-js/static.mjs"),
    ("dynamic",      "bench-js/dynamic.mjs"),
    ("collection",   "bench-js/collection.mjs"),
    ("use_client",   "bench-js/use_client.mjs"),
    ("tla",          "bench-js/tla.mjs"),
];

fn iters_from_env() -> u32 {
    std::env::var("ZFB_SPIKE_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("ZFB_SPIKE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/spike-fixtures"))
}

fn load_fixtures() -> Result<Vec<(&'static str, String)>> {
    let dir = fixtures_dir();
    if !dir.exists() {
        return Err(anyhow!(
            "fixture dir {} missing — run `cargo run -p zfb-runtime-spike --bin zfb-spike-gen-fixtures` first",
            dir.display()
        ));
    }
    FIXTURES
        .iter()
        .map(|(label, rel)| {
            let path = dir.join(rel);
            let src = fs::read_to_string(&path)
                .with_context(|| format!("read fixture {}", path.display()))?;
            Ok((*label, src))
        })
        .collect()
}

fn run_host<H: RenderHost>(mut host: H, fixtures: &[(&'static str, String)]) -> HostMetrics {
    let host_name: &'static str = leak_str(host.name());
    let iters = iters_from_env();
    let mut samples: Vec<MetricSample> = Vec::with_capacity(fixtures.len() * iters as usize);

    for (label, src) in fixtures {
        for i in 0..iters {
            let start = Instant::now();
            let res = host.render_module(RenderInput { name: label, source: src });
            let elapsed = start.elapsed().as_micros();
            match res {
                Ok(out) => {
                    debug_assert!(!out.html.is_empty(), "empty render");
                }
                Err(e) => {
                    eprintln!("[{host_name}] {label} iter {i} FAILED: {e:#}");
                }
            }
            samples.push(MetricSample {
                fixture: label,
                iteration: i,
                render_us: elapsed,
            });
        }
    }
    let rss = current_rss_bytes();
    HostMetrics::from_samples(host_name, samples, rss)
}

fn leak_str(s: &'static str) -> &'static str { s }

fn main() -> Result<()> {
    let fixtures = load_fixtures()?;
    let iters = iters_from_env();
    println!("zfb-runtime-spike bench — {} fixtures × {} iters", fixtures.len(), iters);

    let mut all: Vec<HostMetrics> = Vec::new();

    #[cfg(feature = "quickjs")]
    {
        use zfb_runtime_spike::hosts::quickjs::QuickJsHost;
        match QuickJsHost::new() {
            Ok(h) => all.push(run_host(h, &fixtures)),
            Err(e) => eprintln!("rquickjs init failed: {e:#}"),
        }
    }

    #[cfg(feature = "deno")]
    {
        use zfb_runtime_spike::hosts::deno_core::DenoCoreHost;
        match DenoCoreHost::new() {
            Ok(h) => all.push(run_host(h, &fixtures)),
            Err(e) => eprintln!("deno_core init failed: {e:#}"),
        }
    }

    #[cfg(feature = "ssr-rs")]
    {
        use zfb_runtime_spike::hosts::ssr_rs::SsrRsHost;
        match SsrRsHost::new() {
            Ok(h) => all.push(run_host(h, &fixtures)),
            Err(e) => eprintln!("ssr_rs init failed: {e:#}"),
        }
    }

    if all.is_empty() {
        return Err(anyhow!(
            "no hosts compiled in — build with at least one of --features quickjs|deno|ssr-rs"
        ));
    }

    println!();
    println!(
        "{:<12} {:>14} {:>14} {:>14} {:>14}",
        "host", "cold", "mean (warm)", "p95 (warm)", "rss"
    );
    println!("{}", "-".repeat(72));
    for m in &all {
        println!(
            "{:<12} {:>14} {:>14} {:>14} {:>14}",
            m.host,
            fmt_us(m.cold_start_us),
            fmt_us(m.steady_mean_us),
            fmt_us(m.steady_p95_us),
            fmt_bytes(m.steady_rss_bytes),
        );
    }

    let report_path = std::env::var_os("ZFB_SPIKE_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixtures_dir().join("report.json"));
    fs::write(&report_path, serde_json::to_vec_pretty(&all)?)?;
    println!("wrote report → {}", report_path.display());

    Ok(())
}
