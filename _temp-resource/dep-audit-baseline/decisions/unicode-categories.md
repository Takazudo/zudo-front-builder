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
replacement is seventeen rustfmt-formatted non-test production lines and stays
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

Every disagreement is enumerated below. Consecutive scalars are compressed into
a range only when their old/new direction and new category are identical:

- `U+061D` — `false->true OtherPunctuation`
- `U+07FE..U+07FF` — `false->true CurrencySymbol`
- `U+0888` — `false->true ModifierSymbol`
- `U+09FD` — `false->true OtherPunctuation`
- `U+0A76` — `false->true OtherPunctuation`
- `U+0C77` — `false->true OtherPunctuation`
- `U+0C84` — `false->true OtherPunctuation`
- `U+0D4F` — `false->true OtherSymbol`
- `U+1B4E..U+1B4F` — `false->true OtherPunctuation`
- `U+1B7D..U+1B7F` — `false->true OtherPunctuation`
- `U+20BF..U+20C0` — `false->true CurrencySymbol`
- `U+23FB..U+23FF` — `false->true OtherSymbol`
- `U+2427..U+2429` — `false->true OtherSymbol`
- `U+2B97` — `false->true OtherSymbol`
- `U+2BBA..U+2BBC` — `false->true OtherSymbol`
- `U+2BC9` — `false->true OtherSymbol`
- `U+2BD2..U+2BEB` — `false->true OtherSymbol`
- `U+2BF0..U+2BFF` — `false->true OtherSymbol`
- `U+2E43..U+2E4F` — `false->true OtherPunctuation`
- `U+2E50..U+2E51` — `false->true OtherSymbol`
- `U+2E52..U+2E54` — `false->true OtherPunctuation`
- `U+2E55` — `false->true OpenPunctuation`
- `U+2E56` — `false->true ClosePunctuation`
- `U+2E57` — `false->true OpenPunctuation`
- `U+2E58` — `false->true ClosePunctuation`
- `U+2E59` — `false->true OpenPunctuation`
- `U+2E5A` — `false->true ClosePunctuation`
- `U+2E5B` — `false->true OpenPunctuation`
- `U+2E5C` — `false->true ClosePunctuation`
- `U+2E5D` — `false->true DashPunctuation`
- `U+2FFC..U+2FFF` — `false->true OtherSymbol`
- `U+31E4..U+31E5` — `false->true OtherSymbol`
- `U+31EF` — `false->true OtherSymbol`
- `U+32FF` — `false->true OtherSymbol`
- `U+AB6A..U+AB6B` — `false->true ModifierSymbol`
- `U+FBC2` — `false->true ModifierSymbol`
- `U+FD40..U+FD4F` — `false->true OtherSymbol`
- `U+FDCF` — `false->true OtherSymbol`
- `U+FDFE..U+FDFF` — `false->true OtherSymbol`
- `U+1018D..U+1018E` — `false->true OtherSymbol`
- `U+1019C` — `false->true OtherSymbol`
- `U+10D6E` — `false->true DashPunctuation`
- `U+10D8E..U+10D8F` — `false->true MathSymbol`
- `U+10EAD` — `false->true DashPunctuation`
- `U+10F55..U+10F59` — `false->true OtherPunctuation`
- `U+10F86..U+10F89` — `false->true OtherPunctuation`
- `U+111C9` — `true->false NonspacingMark`
- `U+113D4..U+113D5` — `false->true OtherPunctuation`
- `U+113D7..U+113D8` — `false->true OtherPunctuation`
- `U+1144B..U+1144F` — `false->true OtherPunctuation`
- `U+1145A..U+1145B` — `false->true OtherPunctuation`
- `U+1145D` — `false->true OtherPunctuation`
- `U+11660..U+1166C` — `false->true OtherPunctuation`
- `U+116B9` — `false->true OtherPunctuation`
- `U+1183B` — `false->true OtherPunctuation`
- `U+11944..U+11946` — `false->true OtherPunctuation`
- `U+119E2` — `false->true OtherPunctuation`
- `U+11A3F..U+11A46` — `false->true OtherPunctuation`
- `U+11A9A..U+11A9C` — `false->true OtherPunctuation`
- `U+11A9E..U+11AA2` — `false->true OtherPunctuation`
- `U+11B00..U+11B09` — `false->true OtherPunctuation`
- `U+11BE1` — `false->true OtherPunctuation`
- `U+11C41..U+11C45` — `false->true OtherPunctuation`
- `U+11C70..U+11C71` — `false->true OtherPunctuation`
- `U+11EF7..U+11EF8` — `false->true OtherPunctuation`
- `U+11F43..U+11F4F` — `false->true OtherPunctuation`
- `U+11FD5..U+11FDC` — `false->true OtherSymbol`
- `U+11FDD..U+11FE0` — `false->true CurrencySymbol`
- `U+11FE1..U+11FF1` — `false->true OtherSymbol`
- `U+11FFF` — `false->true OtherPunctuation`
- `U+12FF1..U+12FF2` — `false->true OtherPunctuation`
- `U+16D6D..U+16D6F` — `false->true OtherPunctuation`
- `U+16E97..U+16E9A` — `false->true OtherPunctuation`
- `U+16FE2` — `false->true OtherPunctuation`
- `U+1CC00..U+1CCEF` — `false->true OtherSymbol`
- `U+1CD00..U+1CEB3` — `false->true OtherSymbol`
- `U+1CF50..U+1CFC3` — `false->true OtherSymbol`
- `U+1D1E9..U+1D1EA` — `false->true OtherSymbol`
- `U+1E14F` — `false->true OtherSymbol`
- `U+1E2FF` — `false->true CurrencySymbol`
- `U+1E5FF` — `false->true OtherPunctuation`
- `U+1E95E..U+1E95F` — `false->true OtherPunctuation`
- `U+1ECAC` — `false->true OtherSymbol`
- `U+1ECB0` — `false->true CurrencySymbol`
- `U+1ED2E` — `false->true OtherSymbol`
- `U+1F10D..U+1F10F` — `false->true OtherSymbol`
- `U+1F12F` — `false->true OtherSymbol`
- `U+1F16C..U+1F16F` — `false->true OtherSymbol`
- `U+1F19B..U+1F1AD` — `false->true OtherSymbol`
- `U+1F23B` — `false->true OtherSymbol`
- `U+1F260..U+1F265` — `false->true OtherSymbol`
- `U+1F57A` — `false->true OtherSymbol`
- `U+1F5A4` — `false->true OtherSymbol`
- `U+1F6D1..U+1F6D7` — `false->true OtherSymbol`
- `U+1F6DC..U+1F6DF` — `false->true OtherSymbol`
- `U+1F6F4..U+1F6FC` — `false->true OtherSymbol`
- `U+1F774..U+1F776` — `false->true OtherSymbol`
- `U+1F77B..U+1F77F` — `false->true OtherSymbol`
- `U+1F7D5..U+1F7D9` — `false->true OtherSymbol`
- `U+1F7E0..U+1F7EB` — `false->true OtherSymbol`
- `U+1F7F0` — `false->true OtherSymbol`
- `U+1F8B0..U+1F8BB` — `false->true OtherSymbol`
- `U+1F8C0..U+1F8C1` — `false->true OtherSymbol`
- `U+1F900..U+1F90F` — `false->true OtherSymbol`
- `U+1F919..U+1F97F` — `false->true OtherSymbol`
- `U+1F985..U+1F9BF` — `false->true OtherSymbol`
- `U+1F9C1..U+1FA53` — `false->true OtherSymbol`
- `U+1FA60..U+1FA6D` — `false->true OtherSymbol`
- `U+1FA70..U+1FA7C` — `false->true OtherSymbol`
- `U+1FA80..U+1FA89` — `false->true OtherSymbol`
- `U+1FA8F..U+1FAC6` — `false->true OtherSymbol`
- `U+1FACE..U+1FADC` — `false->true OtherSymbol`
- `U+1FADF..U+1FAE9` — `false->true OtherSymbol`
- `U+1FAF0..U+1FAF8` — `false->true OtherSymbol`
- `U+1FB00..U+1FB92` — `false->true OtherSymbol`
- `U+1FB94..U+1FBEF` — `false->true OtherSymbol`

The delta is accepted. Directive names now follow the maintained Unicode 16
general-category data: newer punctuation and symbols terminate a directive name,
while U+111C9 may occur within one. Following the current Unicode category table
is preferable to indefinitely preserving an undeclared legacy snapshot. The new
CJK/punctuation boundary test covers both ideographic punctuation and newly
recognized U+061D, pinning the parser-level contract and the accepted update.

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
