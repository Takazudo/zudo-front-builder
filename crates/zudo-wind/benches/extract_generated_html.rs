//! Run with `cargo bench -p zudo-wind --bench extract_generated_html`.
//! Reports single-file scaling and the same 5,000 records in five files.
use std::hint::black_box;
use std::time::{Duration, Instant};
use zudo_wind::{extract_candidates, SourceKind};

fn source(start: usize, records: usize) -> String {
    let mut source = String::from("export default [");
    for index in start..start + records {
        if index != start {
            source.push(',');
        }
        let html =
            format!("<span>guide — 日本語 \\n</span><span class=\"line block\">{index}</span>");
        source.push_str("{\"body\":");
        source.push_str(&serde_json::to_string(&html).unwrap());
        source.push('}');
    }
    source.push_str("];\n");
    source
}

fn measure(sources: &[String]) -> Duration {
    // Keep parsing and result creation in the timed loop, like the CLI source pass.
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        for source in sources {
            black_box(extract_candidates(
                black_box(source.as_bytes()),
                SourceKind::Ts,
            ));
        }
        best = best.min(started.elapsed());
    }
    best
}

fn main() {
    for count in [100, 1_000, 5_000] {
        let input = source(0, count);
        let elapsed = measure(std::slice::from_ref(&input));
        println!(
            "{count:>5} records, {:>7} bytes, one file: {:>8.3} ms",
            input.len(),
            elapsed.as_secs_f64() * 1_000.0
        );
    }
    let chunks: Vec<_> = (0..5).map(|chunk| source(chunk * 1_000, 1_000)).collect();
    let elapsed = measure(&chunks);
    println!(
        "5,000 records, {:>7} bytes, five files: {:>8.3} ms",
        chunks.iter().map(String::len).sum::<usize>(),
        elapsed.as_secs_f64() * 1_000.0
    );
}
