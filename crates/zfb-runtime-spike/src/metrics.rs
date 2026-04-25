//! Lightweight metrics used by the bench binary. Kept allocation-free in the
//! hot loop so we measure the runtime, not the harness.

use std::time::Duration;

/// One timed observation.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct MetricSample {
    pub fixture: &'static str,
    pub iteration: u32,
    /// Wall-clock duration of `RenderHost::render_module` for this iteration.
    pub render_us: u128,
}

/// All measurements for one host across all fixtures + iterations.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostMetrics {
    pub host: &'static str,
    /// First render across the whole bench — the "cold start" number.
    pub cold_start_us: u128,
    /// Steady-state per-render mean over iterations >= 2.
    pub steady_mean_us: u128,
    /// Steady-state per-render p95.
    pub steady_p95_us: u128,
    /// Total renders run.
    pub total_renders: u32,
    /// Resident set size in bytes after the bench loop, sampled via
    /// platform-specific syscall. 0 if unavailable.
    pub steady_rss_bytes: u64,
    pub samples: Vec<MetricSample>,
}

impl HostMetrics {
    pub fn from_samples(host: &'static str, samples: Vec<MetricSample>, rss: u64) -> Self {
        let total = samples.len() as u32;
        let cold = samples.first().map(|s| s.render_us).unwrap_or(0);

        let mut warm: Vec<u128> =
            samples.iter().skip(1).map(|s| s.render_us).collect();
        warm.sort_unstable();
        let mean = if warm.is_empty() {
            0
        } else {
            warm.iter().sum::<u128>() / warm.len() as u128
        };
        let p95 = if warm.is_empty() {
            0
        } else {
            let idx = ((warm.len() as f64) * 0.95).ceil() as usize - 1;
            warm[idx.min(warm.len() - 1)]
        };

        Self {
            host,
            cold_start_us: cold,
            steady_mean_us: mean,
            steady_p95_us: p95,
            total_renders: total,
            steady_rss_bytes: rss,
            samples,
        }
    }
}

/// Best-effort RSS sample. Returns 0 if the platform query fails — the bench
/// continues; the report just omits memory data for that run.
pub fn current_rss_bytes() -> u64 {
    // macOS / Linux only — we read /proc on Linux, mach task_info on macOS.
    // For the spike we keep this trivially portable: shell out to `ps -o rss=`
    // for our own pid. Slow (~10ms) but only called once per host run.
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok();
    out.and_then(|o| {
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        s.parse::<u64>().ok().map(|kb| kb * 1024)
    })
    .unwrap_or(0)
}

/// Convert microseconds to a fixed-width display string.
pub fn fmt_us(us: u128) -> String {
    if us == 0 {
        "—".to_string()
    } else if us < 1000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

pub fn fmt_bytes(b: u64) -> String {
    if b == 0 {
        "—".to_string()
    } else if b < 1024 {
        format!("{b}B")
    } else if b < 1024 * 1024 {
        format!("{:.1}KB", b as f64 / 1024.0)
    } else {
        format!("{:.1}MB", b as f64 / (1024.0 * 1024.0))
    }
}

pub fn fmt_duration(d: Duration) -> String {
    fmt_us(d.as_micros())
}
