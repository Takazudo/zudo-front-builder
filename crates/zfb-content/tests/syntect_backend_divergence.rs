//! Syntect regex-backend divergence fixtures (zfb#1573).
//!
//! Highlights a small fixture corpus through [`zfb_content::syntect_highlight`]
//! and compares the output against snapshots committed from the `regex-onig`
//! backend (`tests/fixtures/syntect_backend/`). Two legs:
//!
//! * **onig leg** (`feature = "syntect-onig"`, the default — this is what the
//!   T1 gate runs): exact snapshot match. A real regression guard proving the
//!   backend-selection restructure keeps native highlight output
//!   byte-identical.
//! * **fancy leg** (`syntect-fancy` active WITHOUT `syntect-onig`):
//!   INFORMATIONAL, NON-GATING per zfb#1573 — it reports fancy-vs-onig
//!   grammar divergences to stderr and always passes. Divergences here are
//!   documentation for the wasm parity work (zfb#1578), not failures.
//!
//! ## Why the fancy leg cannot run via `cargo test -p zfb-content`
//!
//! The `zfb-render` dev-dependency depends back on this crate with default
//! features, and cargo unifies features across the dev-dep cycle: any
//! `cargo test -p zfb-content --no-default-features --features syntect-fancy`
//! still resolves `zfb-content` with `syntect-onig` enabled, and syntect gives
//! regex-onig precedence when both backends are on (syntect-5.3.0
//! src/parsing/regex.rs:191). The fancy leg therefore only executes from a
//! consumer graph without that cycle — e.g. an out-of-workspace crate
//! depending on `zfb-content` with `default-features = false,
//! features = ["syntect-fancy"]`, or the future `zfb-md-wasm` crate
//! (zfb#1576). The zfb#1573 verification ran exactly that external
//! comparison; result: all fixtures below were byte-identical between the
//! two backends.
//!
//! ## Snapshot regeneration
//!
//! ```sh
//! ZFB_UPDATE_SYNTECT_BACKEND_SNAPSHOTS=1 \
//!   cargo test -p zfb-content --test syntect_backend_divergence
//! ```
//!
//! (Only the onig leg writes snapshots — they are the oracle.)

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use zfb_content::syntect_highlight::Highlighter;

/// `(name, lang, source)` fixture corpus. Deliberately small (per zfb#1573)
/// but regex-heavy: multi-line strings, block comments, regex literals,
/// heredocs and nested interpolation are where oniguruma and fancy-regex are
/// most likely to disagree.
const FIXTURES: &[(&str, &str, &str)] = &[
    (
        "rust",
        "rust",
        r#"/// Doc comment with `code`.
pub fn greet(name: &str) -> String {
    let template = format!("hello {name}");
    /* block
       comment */
    template.to_uppercase()
}
"#,
    ),
    (
        "javascript",
        "js",
        r#"const re = /ab+c\/[^d]/gi;
async function load({ url = "https://example.com" } = {}) {
  const body = `template ${url} literal
spanning lines`;
  return fetch(url).then((r) => r.json());
}
"#,
    ),
    (
        "css",
        "css",
        r#"@media (min-width: 40rem) {
  .card__title:hover::after {
    content: "\2192";
    color: rgb(128 0 255 / 0.5);
  }
}
"#,
    ),
    (
        "html",
        "html",
        r#"<!DOCTYPE html>
<html lang="en">
  <body data-x='1 > 0 &amp; "quoted"'>
    <!-- comment -->
    <script>const x = 1 < 2;</script>
  </body>
</html>
"#,
    ),
    (
        "python",
        "python",
        r#"import re

PATTERN = re.compile(r"^(?P<key>\w+)\s*=\s*(.+)$")

def parse(text: str) -> dict:
    """Docstring
    over two lines."""
    return {m["key"]: m[2] for m in PATTERN.finditer(text)}
"#,
    ),
    (
        "bash",
        "bash",
        r#"#!/usr/bin/env bash
set -euo pipefail
for f in "$@"; do
  cat <<EOF
name: ${f%%.*}
EOF
done
"#,
    ),
    (
        "typescript",
        "ts",
        r#"@sealed
class Repository<T extends { id: string }> {
  find<U extends T>(value: U): value is U & { active: true } {
    const label = `record ${value.id}
active`;
    return label.length > 0 && "active" in value;
  }
}

enum Mode { Read = "read", Write = "write" }
const map = <T, U>(items: T[], fn: (item: T) => U): U[] => items.map(fn);
"#,
    ),
    (
        "tsx",
        "tsx",
        r#"type CardProps<T> = { item: T; title: string; active?: boolean };

export const Card = <T extends { id: string }>({ item, ...props }: CardProps<T>) => (
  <>
    <article {...props} data-id={item.id}>
      <h2>{props.title}</h2>
      {props.active && <span>{`active ${item.id}`}</span>}
    </article>
  </>
);
"#,
    ),
];

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("syntect_backend")
}

/// Render one fixture through both public highlight modes and concatenate the
/// results into a single comparable document.
fn render_fixture(hl: &Highlighter, lang: &str, source: &str) -> String {
    let styled = hl
        .highlight(source, Some(lang), None)
        .expect("styled highlight should not error on fixture input");
    let classes = hl
        .highlight_lines_classes(source, Some(lang), "hi-", &BTreeMap::new())
        .expect("class highlight should not error on fixture input");
    assert!(
        !classes.fallback,
        "fixture lang {lang:?} must resolve to a real syntax — a fallback \
         snapshot would compare escaped text, not backend tokenization"
    );
    format!(
        "== styled ==\n{styled}\n== classes ==\n{}",
        classes.lines.concat()
    )
}

/// Onig leg: exact snapshot match (regression guard for native byte-identity).
///
/// Guarded on the ACTIVE backend, not just the requested feature: when both
/// backend features are unified on, syntect uses onig, so the snapshots are
/// still the correct oracle.
#[cfg(feature = "syntect-onig")]
#[test]
fn onig_output_matches_committed_snapshots() {
    let hl = Highlighter::new();
    let update = std::env::var_os("ZFB_UPDATE_SYNTECT_BACKEND_SNAPSHOTS").is_some();
    let dir = snapshot_dir();
    if update {
        fs::create_dir_all(&dir).expect("create snapshot dir");
    }
    for (name, lang, source) in FIXTURES {
        let rendered = render_fixture(&hl, lang, source);
        let path = dir.join(format!("{name}.html"));
        if update {
            fs::write(&path, &rendered).expect("write snapshot");
            continue;
        }
        let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "missing snapshot {} ({e}); regenerate with \
                 ZFB_UPDATE_SYNTECT_BACKEND_SNAPSHOTS=1",
                path.display()
            )
        });
        assert_eq!(
            rendered, expected,
            "onig highlight output diverged from committed snapshot for \
             fixture {name:?}"
        );
    }
}

/// Fancy leg: INFORMATIONAL, NON-GATING (zfb#1573). Compares fancy-regex
/// output against the committed onig oracle and reports divergences to stderr
/// without failing — grammar divergence between the backends is data for the
/// wasm parity gate (zfb#1578), not a regression.
///
/// See the module docs for why this leg is unreachable from in-workspace
/// `cargo test` invocations (zfb-render dev-dep cycle re-enables onig).
#[cfg(all(feature = "syntect-fancy", not(feature = "syntect-onig")))]
#[test]
fn fancy_vs_onig_divergence_informational() {
    let hl = Highlighter::new();
    let dir = snapshot_dir();
    let mut divergent = 0usize;
    for (name, lang, source) in FIXTURES {
        let rendered = render_fixture(&hl, lang, source);
        let path = dir.join(format!("{name}.html"));
        let Ok(oracle) = fs::read_to_string(&path) else {
            eprintln!(
                "[syntect-backend] {name}: no onig oracle at {}",
                path.display()
            );
            continue;
        };
        if rendered == oracle {
            eprintln!("[syntect-backend] {name}: fancy == onig");
        } else {
            divergent += 1;
            eprintln!("[syntect-backend] {name}: DIVERGES from onig oracle");
            for (i, (f, o)) in rendered.lines().zip(oracle.lines()).enumerate() {
                if f != o {
                    eprintln!("  line {}:\n    onig:  {o}\n    fancy: {f}", i + 1);
                }
            }
        }
    }
    eprintln!(
        "[syntect-backend] {divergent}/{} fixtures diverge (informational — \
         not a gate, see zfb#1573)",
        FIXTURES.len()
    );
}
