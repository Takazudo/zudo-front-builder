# `unicode_categories` decision

## Verdict

**MIGRATE** from unmaintained `unicode_categories` 0.1.1 to maintained
`unicode-general-category` 1.1.0. The only used predicates were
`UnicodeCategories::is_punctuation` and `UnicodeCategories::is_symbol`, combined
as the Unicode general-category class `P | S` in `directive_parser.rs`.

Inlining was rejected because remark-directive's name grammar deliberately uses
the full Unicode punctuation and symbol classes. Preserving that contract would
require a hand-maintained Unicode table, which is the issue's immediate-KEEP
trigger. `unicode-ident` does not expose the required general-category semantics;
`unicode-general-category` does, with a generated Unicode 16.0.0 table. The
replacement is sixteen rustfmt-formatted non-test production lines and stays
below the 40-line ceiling.

## Differential

A throwaway Cargo binary outside the worktree depended on both exact versions,
iterated `0x000000..=0x10FFFF`, skipped surrogate code points with
`char::from_u32`, and compared:

- old: `ch.is_punctuation() || ch.is_symbol()`;
- new: whether `get_general_category(ch).abbreviation()` starts with `P` or `S`.

It evaluated all **1,112,064 Unicode scalar values** and printed every differing
code point with the old result, new result, and new category. The complete output
had SHA-256
`f8bf6ae0c6ecd40b485dc3a75ab4381cc745999ad43258bb038e5aacd9ba6bdb`.

The result was **1,855 disagreements**:

- **1,854** scalars are newly recognized as punctuation/symbol boundaries:
  1,698 `So`, 129 `Po`, 10 `Sc`, 4 `Pe`, 4 `Ps`, 4 `Sk`, 3 `Pd`, and 2 `Sm`.
- **U+111C9 SHARADA SANDHI MARK** is the sole removal: the old table treated it
  as punctuation, while Unicode 16 classifies it as `Mn` (Nonspacing Mark).

The delta is accepted. Directive names now follow the maintained Unicode 16
general-category data: newer punctuation and symbols terminate a directive name,
while U+111C9 may occur within one. Following the current Unicode category table
is preferable to indefinitely preserving an undeclared legacy snapshot, and the
new CJK/ideographic-punctuation boundary test pins the parser-level contract.

## Dependency and lock impact

The lockfile removes `unicode_categories` 0.1.1 and adds
`unicode-general-category` 1.1.0. Both are leaf packages, so the transitive
package-count delta is zero (one removed, one added) and there are no transitive
dependency changes. The license changes from `MIT OR Apache-2.0` to
`Apache-2.0`; `cargo deny check` validates the resulting graph.

## Behavior and changelog decision

Because the differential is nonempty, this is package-facing. The Unicode-table
update is recorded in the v2.14.0 `zfb` and `zfb-md-wasm` lanes. The
`zfb-runtime`, `zfb-adapter-cloudflare`, and `create-zfb` lanes contain exactly
`- No package-specific changes.`

## Verification

- PASS — `cargo test -p zfb-content` (isolated target; no existing assertion
  edits). The first attempt hit a host linker sandbox denial; an exact retry
  passed all unit, integration, and doc tests.
- PASS — `cargo check --target wasm32-unknown-unknown -p zfb-md-wasm`. The host
  put Homebrew Cargo/Rust 1.94 ahead of rustup stable 1.96, so the target was not
  visible until the matching rustup Cargo and Rust compiler were selected.
- PASS — `cargo deny check` (`advisories ok, bans ok, licenses ok, sources ok`;
  only the repository's configured warnings were emitted).
- PASS — `git diff --check`.
