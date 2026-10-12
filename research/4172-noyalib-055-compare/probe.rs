//! #4172 probe, copied by `compare.sh` into a scratch checkout as
//! `crates/zfb-content/examples/yaml_4172_probe.rs`. It never ships.
//!
//! Every input goes through `zfb_content::frontmatter::extract`, the real
//! `serde_yaml::from_str::<serde_json::Value>` call site, with the parser
//! configuration unchanged.
//!
//! `observe` prints one JSON line per semantic boundary case so the two pins
//! can be diffed; it asserts nothing. `flow <shape> <scalars>` parses one long
//! flow line once and prints its timing; the driver owns repetitions and
//! limits.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use zfb_content::frontmatter::extract;
use zfb_content::FrontmatterError;

fn document(yaml: &str) -> String {
    format!("---\n{yaml}---\nbody\n")
}

fn observe_one(yaml: &str) -> Value {
    match extract(Path::new("probe.md"), &document(yaml)) {
        Ok(fm) => json!({ "kind": "success", "value": fm.value }),
        Err(err) => {
            let location = match &err {
                FrontmatterError::Yaml(yaml_err) => yaml_err
                    .location()
                    .map(|l| json!({ "line": l.line(), "column": l.column(), "index": l.index() })),
                _ => None,
            };
            let display = match &err {
                FrontmatterError::Yaml(yaml_err) => yaml_err.to_string(),
                other => other.to_string(),
            };
            json!({
                "kind": "error",
                "display": display,
                "frontmatterDisplay": err.to_string(),
                "location": location,
            })
        }
    }
}

/// A flow-sequence key of exactly `chars` characters: `[a, a, …, a]`
/// (the shape of noyalib 0.0.55's `implicit_key_length` conformance test).
fn seq_key(chars: usize) -> String {
    assert!(chars >= 3 && chars % 3 == 0);
    format!("[{}a]", "a, ".repeat(chars / 3 - 1))
}

fn semantic_cases() -> Vec<(&'static str, String)> {
    vec![
        // 0.0.55 "breaking parse behaviour": verbatim tags name the URI exactly.
        ("verbatim-int", "v: !<int> 1\n".into()),
        (
            "verbatim-core-uri-int",
            "v: !<tag:yaml.org,2002:int> 7\n".into(),
        ),
        (
            "verbatim-core-uri-str",
            "v: !<tag:yaml.org,2002:str> 7\n".into(),
        ),
        ("verbatim-local", "v: !<!foo> x\n".into()),
        ("verbatim-uri", "v: !<foo> x\n".into()),
        ("shorthand-local-control", "v: !foo x\n".into()),
        ("secondary-shorthand-control", "v: !!int '5'\n".into()),
        // 0.0.55 "breaking parse behaviour": implicit flow-collection keys
        // are limited to 1024 characters (YAML 1.2.2 §8.2.2 / §7.4.2).
        (
            "flow-seq-key-1024-chars",
            format!("{}    : v\n", seq_key(1020)),
        ),
        (
            "flow-seq-key-1025-chars",
            format!("{}     : v\n", seq_key(1020)),
        ),
        (
            "flow-seq-key-30000-chars",
            format!("{}: v\n", seq_key(30_000)),
        ),
        ("flow-map-key-short", "{a: 1}: v\n".into()),
        (
            "flow-pair-in-seq-900",
            format!("k: [{}: v]\n", seq_key(900)),
        ),
        (
            "flow-pair-in-seq-1050",
            format!("k: [{}: v]\n", seq_key(1050)),
        ),
        (
            "flow-mapping-long-key",
            format!("k: {{{}: v}}\n", seq_key(30_000)),
        ),
        ("explicit-long-key", format!("? {}\n: v\n", seq_key(3_000))),
        ("multi-line-flow-key", "[a,\n a]: v\n".into()),
        (
            "long-plain-scalar-key",
            format!("{}: v\n", "k".repeat(5_000)),
        ),
        (
            "long-quoted-scalar-key",
            format!("\"{}\": v\n", "k".repeat(5_000)),
        ),
        // 0.0.55 refuses duplicate keys only for a `noyalib::Value` target;
        // zfb deserializes into `serde_json::Value`.
        ("duplicate-top-level-control", "a: 1\na: 2\n".into()),
        ("duplicate-nested", "m:\n  a: 1\n  a: 2\n".into()),
        ("duplicate-in-flow-mapping", "m: {a: 1, a: 2}\n".into()),
        // Flow collection at document start is now streamed, not buffered.
        ("flow-seq-at-document-start", "[1, [2, 3], {a: b}]\n".into()),
        (
            "flow-strings-with-indicators",
            "k: [\"a,b\", \"[c]\", \"{d}\", 'e: f']\n".into(),
        ),
        (
            "merge-key-ordinary-control",
            "d: &d {x: 1}\np:\n  <<: *d\n".into(),
        ),
        // 0.0.55 "every loader enforces every limit": an alias can no longer
        // build a value deeper than `max_depth` (128 under the compat profile).
        (
            "alias-depth-under-max-control",
            format!("a: &x {}\nb: {}\n", nested(40, "1"), nested(40, "*x")),
        ),
        (
            "alias-depth-over-max",
            format!("a: &x {}\nb: {}\n", nested(70, "1"), nested(70, "*x")),
        ),
    ]
}

/// `depth` nested flow sequences around `inner`: `[[…inner…]]`.
fn nested(depth: usize, inner: &str) -> String {
    format!("{}{inner}{}", "[".repeat(depth), "]".repeat(depth))
}

/// One line of nested flow sequences holding `scalars` one-byte scalars, the
/// shape upstream's `scanner_complexity` test uses.
fn long_flow_line(scalars: usize) -> String {
    assert!(scalars >= 1_000 && scalars % 1_000 == 0);
    let inner = format!("[{}a]", "a,".repeat(999));
    format!("[{}]", vec![inner; scalars / 1_000].join(","))
}

fn flow(shape: &str, scalars: usize) -> Value {
    let line = long_flow_line(scalars);
    let mut yaml = String::new();
    match shape {
        "mapping-value" => writeln!(yaml, "k: {line}").unwrap(),
        "document-start" => writeln!(yaml, "{line}").unwrap(),
        other => panic!("unknown flow shape {other:?}"),
    }
    let source = document(&yaml);
    let started = Instant::now();
    let result = extract(Path::new("probe.md"), &source);
    let elapsed = started.elapsed();
    let (ok, shape_ok, error) = match &result {
        Ok(fm) => {
            let outer = match shape {
                "mapping-value" => &fm.value["k"],
                _ => &fm.value,
            };
            let shape_ok = outer.as_array().is_some_and(|outer| {
                outer.len() == scalars / 1_000
                    && outer
                        .iter()
                        .all(|inner| inner.as_array().is_some_and(|i| i.len() == 1_000))
            });
            (true, shape_ok, None)
        }
        Err(err) => (false, false, Some(err.to_string())),
    };
    json!({
        "shape": shape,
        "scalars": scalars,
        "inputBytes": source.len(),
        "ok": ok,
        "shapeOk": shape_ok,
        "error": error,
        "elapsedNanos": elapsed.as_nanos() as u64,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["observe"] => {
            for (name, yaml) in semantic_cases() {
                let line = json!({
                    "name": name,
                    "yamlBytes": yaml.len(),
                    "observation": observe_one(&yaml),
                });
                println!("{line}");
            }
        }
        ["flow", shape, scalars] => {
            let scalars: usize = scalars.parse().expect("scalar count");
            println!("{}", flow(shape, scalars));
        }
        _ => {
            eprintln!(
                "usage: yaml_4172_probe observe | flow <mapping-value|document-start> <scalars>"
            );
            std::process::exit(2);
        }
    }
}
