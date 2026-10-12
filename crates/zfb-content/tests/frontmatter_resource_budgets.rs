//! Pins the YAML resource budgets that apply to `.md`/`.mdx` frontmatter.
//!
//! Frontmatter goes through `serde_yaml::from_str::<serde_json::Value>`
//! (the `noyalib-serde-yaml` alias), which uses noyalib's `serde_yaml_compat`
//! profile: `alias_anchor_ratio` 10.0 and `max_nodes` 250,000 among others.
//! RUSTSEC-2026-0333 (#4159) made noyalib 0.0.53 charge those budgets on the
//! typed path too; this target already hit them at 0.0.44, so the boundaries
//! below hold on both pins. A future pin that moves a boundary or rewords
//! the rejection fails here and needs a recorded decision in DEPENDENCIES.md.

use std::fmt::Write as _;
use std::path::Path;

use zfb_content::frontmatter::extract;
use zfb_content::FrontmatterError;

fn parse(yaml: &str) -> Result<serde_json::Value, FrontmatterError> {
    let source = format!("---\n{yaml}---\nbody\n");
    extract(Path::new("post.md"), &source).map(|fm| fm.value)
}

fn yaml_error(yaml: &str) -> serde_yaml::Error {
    match parse(yaml) {
        Err(FrontmatterError::Yaml(err)) => err,
        other => panic!("expected a YAML budget error, got {other:?}"),
    }
}

fn one_anchor_with_aliases(aliases: usize) -> String {
    let mut yaml = String::from("base: &b hello\n");
    for i in 0..aliases {
        writeln!(yaml, "k{i}: *b").unwrap();
    }
    yaml
}

fn flow_sequences(outer: usize, inner: usize) -> String {
    let mut yaml = String::new();
    for o in 0..outer {
        write!(yaml, "s{o}: [").unwrap();
        yaml.push_str(&vec!["1"; inner].join(","));
        yaml.push_str("]\n");
    }
    yaml
}

#[test]
fn ten_aliases_per_anchor_are_accepted() {
    let value = parse(&one_anchor_with_aliases(10)).expect("ratio 10 is within budget");
    assert_eq!(value["k9"], "hello");
}

#[test]
fn eleven_aliases_per_anchor_are_rejected() {
    let err = yaml_error(&one_anchor_with_aliases(11));
    assert_eq!(err.to_string(), "repetition limit exceeded");
    // The budget error carries no location, so zfb's diagnostic falls back
    // to line 1, column 1 of the file.
    assert!(err.location().is_none());
    assert_eq!(
        FrontmatterError::Yaml(err).to_string(),
        "invalid YAML in frontmatter: repetition limit exceeded",
    );
}

#[test]
fn alias_ratio_counts_every_anchor_in_the_document() {
    let mut yaml = String::from("a: &a x\nb: &b y\n");
    for i in 0..20 {
        writeln!(yaml, "r{i}: *a").unwrap();
    }
    parse(&yaml).expect("20 aliases over 2 anchors is ratio 10");
    yaml.push_str("r20: *b\n");
    assert_eq!(yaml_error(&yaml).to_string(), "repetition limit exceeded");
}

#[test]
fn node_budget_boundary() {
    parse(&flow_sequences(240, 1_000)).expect("240,240 nodes is within max_nodes");
    let err = yaml_error(&flow_sequences(260, 1_000));
    assert_eq!(
        err.to_string(),
        "max_nodes budget exceeded: observed 250001 > limit 250000"
    );
    assert!(err.location().is_none());
}

#[test]
fn large_realistic_frontmatter_is_accepted() {
    let mut yaml = String::from("title: Big\n");
    writeln!(yaml, "description: \"{}\"", "x".repeat(200_000)).unwrap();
    yaml.push_str("tags:\n");
    for i in 0..2_000 {
        writeln!(yaml, "  - tag-{i}").unwrap();
    }
    yaml.push_str("items:\n");
    for i in 0..500 {
        writeln!(
            yaml,
            "  - id: {i}\n    name: item {i}\n    meta: {{ a: 1, b: [1, 2, 3] }}"
        )
        .unwrap();
    }
    yaml.push_str("defaults: &d\n  layout: post\npage:\n  <<: *d\n");

    let value = parse(&yaml).expect("large frontmatter stays within the default budgets");
    assert_eq!(value["tags"].as_array().unwrap().len(), 2_000);
    assert_eq!(value["items"].as_array().unwrap().len(), 500);
    assert_eq!(value["description"].as_str().unwrap().len(), 200_000);
    // serde_yaml_compat treats `<<` as an ordinary key, not a merge.
    assert_eq!(value["page"]["<<"]["layout"], "post");
}
