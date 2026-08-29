# `serde_yaml` decision

## Verdict

**STAY** on `serde_yaml` 0.9.34+deprecated for now. None of the three required
candidates satisfies all five replacement criteria:

- `serde_yaml_ng` 0.10.0 is behaviorally drop-in and has the same dependency
  subtree, but its latest crates.io release is from 2024-05-26 and its last
  repository commit is from 2025-09-14. As of 2026-08-30, moving to a fork with
  no release in more than two years and no repository activity in nearly a year
  does not resolve the maintenance liability that motivated this issue.
- `serde_yml` 0.0.13 explicitly describes itself as deprecated and
  unmaintained, its repository is archived, `cargo deny` reports
  RUSTSEC-2025-0068, and its parser changes both pinned error locations and YAML
  merge-key transcoding.
- `saphyr` 0.0.12 is genuinely active, but it is a parser/DOM crate rather than
  a Serde deserializer. It has no `from_str::<serde_json::Value>` equivalent and
  therefore cannot be a drop-in replacement.

No production source, manifest, lockfile, assertion, or changelog file is
changed by this decision.

## Existing contract

The dependency is load-bearing in three places:

1. `zfb-content` deserializes all Markdown frontmatter directly into
   `serde_json::Value` and stores `serde_yaml::Error` in `FrontmatterError`.
2. `zfb` calls `Error::location()` and depends on its 1-based line and column,
   then adds one source line for the opening `---` delimiter.
3. `zfb-md-wasm` compiles that path for `wasm32-unknown-unknown` and pins the
   resulting diagnostic line and column.

The evaluation never edited the assertions in
`crates/zfb-content/tests/error_messages.rs` or
`crates/zfb-md-wasm/tests/api.rs`.

## Evaluation method

A throwaway Cargo binary outside the worktree depended on the current
`serde_yaml` 0.9.34+deprecated and all candidates. It compared direct
`from_str::<serde_json::Value>` results for:

- strings, booleans, nulls, integers, floats, sequences, and mappings;
- quoted date-like strings and YAML date-like plain scalars;
- hexadecimal integers and literal block scalars;
- anchors and YAML merge keys;
- Unicode and integers above JavaScript's exact-integer range;
- a boolean-looking mapping key;
- three malformed flow sequences, including their formatted error and
  `(line, column, index)` location.

The same probe graph was checked for `wasm32-unknown-unknown`. Candidate
maintenance was checked against the candidates' crates.io metadata, published
crate manifests/source, and canonical GitHub repository metadata and commits.
The repository's `deny.toml` was applied to candidate probe graphs.

## Candidate comparison

### `serde_yaml_ng` 0.10.0

1. **Location API and semantics: PASS.** The workspace compiles unchanged when
   the dependency is aliased as
   `serde_yaml = { package = "serde_yaml_ng", version = "0.10" }`.
   `Error::location()` and `Location::{line,column,index}` have the same API and
   1-based semantics. The three malformed inputs produced the same complete
   error strings and locations as the current parser: `(2,1,12)`, `(2,6,30)`,
   and `(3,1,14)`.
2. **wasm32 and size: PASS.** Both the isolated candidate probe and
   `cargo check --target wasm32-unknown-unknown -p zfb-md-wasm` passed with the
   rustup toolchain. A full candidate `pnpm test:md-wasm` build passed all 12
   test files / 215 tests. Its gzip-9 sizes compared with
   `baseline-wasm-size.txt` were:

   | artifact       |  baseline | candidate |   delta |
   | -------------- | --------: | --------: | ------: |
   | default        | 1,467,589 | 1,451,654 | -15,935 |
   | highlight-only |   763,804 |   763,808 |      +4 |
   | render-only    | 1,016,073 | 1,016,097 |     +24 |
   | parse-only     |   275,195 |   259,779 | -15,416 |

   The default and parse-only artifacts became smaller; the single-digit and
   24-byte increases are far below the guard. The documented size consistency
   check also passed.

3. **Maintenance: FAIL.** The canonical repository is not archived, but its
   last commit is
   [`3628102977f3`](https://github.com/acatton/serde-yaml-ng/commit/3628102977f3)
   from 2025-09-14. More importantly, crates.io records 0.10.0 as the latest
   release, published 2024-05-26; the repository has not published the later
   work. This is not sufficiently current to replace a deprecated parser on
   maintenance grounds.
4. **JSON transcoding: PASS.** Every probe value was identical, including the
   significant merge-key case: both current `serde_yaml` and `serde_yaml_ng`
   preserved `"<<"` as a JSON key rather than applying a merge.
5. **cargo-deny and dependency delta: PASS.** The lockfile trial was a one-for-
   one rename from `serde_yaml` 0.9.34+deprecated to `serde_yaml_ng` 0.10.0.
   `indexmap`, `itoa`, `ryu`, `serde`, and `unsafe-libyaml` remained identical;
   package count and transitive package count were unchanged. The direct
   license changes from `MIT OR Apache-2.0` to `MIT`. Both the full workspace
   trial and the candidate probe passed `cargo deny check`.

Primary metadata:
[crates.io](https://crates.io/crates/serde_yaml_ng/0.10.0),
[repository](https://github.com/acatton/serde-yaml-ng).

### `serde_yml` 0.0.13

1. **Location API and semantics: FAIL.** Its compatibility shim exposes
   `Error::location()` with 1-based accessors, but the actual positions differ.
   For `title: [oops`, current `serde_yaml` reports `(2,1,12)` while
   `serde_yml` reports `(1,13,12)`. For the two-line malformed collection, the
   current parser reports `(2,6,30)` while `serde_yml` reports `(2,10,34)`.
   Those differences would fail the existing diagnostic assertions.
2. **wasm32 and size: PARTIAL PASS / REJECTED.** The isolated dependency and
   API probe compiled for `wasm32-unknown-unknown`. A workspace size build was
   not justified after the pinned diagnostic and transcoding contracts failed;
   it cannot be a migration candidate regardless of size.
3. **Maintenance: FAIL.** The published crate describes itself as
   "DEPRECATED" and "unmaintained", 0.0.13 is a thin forwarding shim to
   `noyalib`, and the canonical repository is archived. Its 2026-05 release is
   a retirement shim, not resumed maintenance.
4. **JSON transcoding: FAIL.** On the anchor/merge fixture, current
   `serde_yaml` returns `{"copy":{"<<":{"enabled":true}},...}` while
   `serde_yml` applies the merge and returns
   `{"copy":{"enabled":true},...}`. This changes public frontmatter JSON.
5. **cargo-deny and dependency delta: FAIL.** The direct graph replaces the
   current `serde_yaml`/`unsafe-libyaml` parser path with `serde_yml` 0.0.13 and
   `noyalib` 0.0.5, whose direct support dependencies are `indexmap`, `memchr`,
   `rustc-hash`, `serde`, and `smallvec`. Their licenses are allowed
   `MIT OR Apache-2.0`, but the repository deny policy rejects the candidate for
   RUSTSEC-2025-0068 (unsound and unmaintained; no safe upgrade).

Primary metadata:
[crates.io](https://crates.io/crates/serde_yml/0.0.13),
[repository](https://github.com/sebastienrousseau/serde_yml).

### `saphyr` 0.0.12

1. **Location API and semantics: PARTIAL PASS / NOT EQUIVALENT.** Parse errors
   carry `ScanError::marker()` and `Marker::{line,col,index}`; the crate source
   documents line and column as 1-indexed. There is no `serde_yaml::Error`
   equivalent, so adopting this API would require an adapter and altered error
   ownership rather than a dependency alias.
2. **wasm32 and size: PARTIAL PASS / REJECTED.** `saphyr`, its default
   `encoding` feature, and its complete isolated dependency graph compile for
   `wasm32-unknown-unknown`. A workspace size build was not justified because
   the crate cannot perform the required Serde transcoding.
3. **Maintenance: PASS.** The canonical repository is active and not archived.
   Version 0.0.12 was committed and published on 2026-08-18, following 0.0.11
   and 0.0.10 in July 2026.
4. **JSON transcoding: FAIL.** `saphyr` exposes
   `Yaml::load_from_str -> Vec<Yaml>`, not a Serde deserializer. It has no
   `from_str::<serde_json::Value>` API. A manual DOM-to-JSON converter would be
   a new semantic layer and is not drop-in. The separately named
   `serde-saphyr` crate was not one of this issue's candidates and is not an API
   exported by `saphyr`.
5. **cargo-deny and dependency delta: PASS AS A GRAPH, NOT AS A MIGRATION.** The
   isolated graph adds `saphyr`, `saphyr-parser`, `encoding_rs`, `hashlink`,
   `ordered-float`, `arraydeque`, and their already-common support crates
   (`hashbrown`, `num-traits`, `thiserror`). The licenses are combinations of
   MIT, Apache-2.0, and BSD-3-Clause permitted by `deny.toml`; the candidate
   probe passed all four cargo-deny checks. The larger graph cannot be compared
   as a true workspace replacement because a separate Serde adapter would also
   be required.

Primary metadata:
[crates.io](https://crates.io/crates/saphyr/0.0.12),
[repository](https://github.com/saphyr-rs/saphyr).

## Decision reasoning

`serde_yaml_ng` proves that a mechanical, assertion-preserving swap is possible,
but it does not currently prove that the project would gain an actively
maintained parser. Maintenance is one of the five required acceptance criteria,
not a secondary preference. `serde_yml` is strictly worse against that criterion
and introduces behavioral/security failures. `saphyr` is healthy but would turn
this dependency replacement into a parser-adapter project whose JSON and error
semantics would need a separately designed and tested contract.

The least risky current decision is therefore to retain the known,
feature-complete parser, record the blocker precisely, and re-evaluate when a
candidate combines active releases with the proven compatibility surface.

## Verification performed

- PASS — `serde_yaml_ng` trial:
  `cargo test -p zfb-content --test error_messages` (2/2), with existing
  assertions unedited.
- PASS — `serde_yaml_ng` trial:
  `cargo check --target wasm32-unknown-unknown -p zfb-md-wasm` using rustup
  Rust/Cargo 1.96 and the isolated `/tmp/zfb-2750-target`.
- PASS — `serde_yaml_ng` trial: `pnpm test:md-wasm` (12 files, 215 tests) and
  the four measured size outputs above. The first invocation stopped before
  compilation because the installed `wasm-bindgen` CLI was absent from `PATH`;
  exposing the existing pinned 0.2.121 binary made the exact retry pass.
- PASS — `node scripts/assert-md-wasm-size-docs.mjs` (8 files match the shipped
  size manifest).
- PASS — full workspace `cargo deny check` during the `serde_yaml_ng` trial.
- PASS — isolated `serde_yaml_ng` + `saphyr` candidate graph under the
  repository `deny.toml`.
- EXPECTED REJECTION — isolated `serde_yml` graph: RUSTSEC-2025-0068.
- PASS — all three candidates compile in the isolated
  `wasm32-unknown-unknown` dependency probe.

The heavier `zfb` diagnostic test trial encountered the host's known
`Operation not permitted` archive/link failure while building `zfb-islands`;
the md-wasm lane subsequently compiled the same frontmatter/diagnostic path and
passed its pinned line/column assertion. No assertion was changed.

## Required follow-up issue (must be filed before merge)

The implementation owner must file the following issue before merging this
evaluation. This report deliberately does not create GitHub state.

### Title

`[Dep Audit][Follow-up] Re-evaluate serde_yaml when a maintained compatible replacement ships`

### Body

```markdown
## Context

Follow-up to #2750.

The #2750 evaluation retained `serde_yaml` 0.9.34+deprecated because none of
the required candidates satisfied maintenance, diagnostic compatibility,
Serde-to-JSON compatibility, wasm32, and cargo-deny at the same time.

The closest candidate was `serde_yaml_ng` 0.10.0. It was mechanically drop-in:
the existing error assertions passed unedited, malformed-YAML error strings and
1-based `(line, column, index)` locations matched, representative
`from_str::<serde_json::Value>` results matched, wasm32 and the complete
md-wasm lane passed, its dependency subtree was identical, and wasm sizes stayed
within the guard. However, its latest crate was published on 2024-05-26 and its
last repository commit was 2025-09-14. As of the 2026-08-30 evaluation, adopting
it would replace an archived parser with an effectively stale fork rather than
resolve the maintenance liability.

`serde_yml` 0.0.13 is deprecated, archived, rejected by RUSTSEC-2025-0068,
changes pinned error locations, and changes merge-key transcoding. `saphyr`
0.0.12 is active but is not a Serde deserializer and has no
`from_str::<serde_json::Value>` replacement.

The durable comparison is in
`_temp-resource/dep-audit-baseline/decisions/serde-yaml.md`.

## Re-evaluation trigger

Re-open the replacement decision when at least one of these is true:

- `serde_yaml_ng` publishes a release containing its post-0.10 repository work
  and demonstrates ongoing maintenance;
- another maintained fork exposes the same `serde_yaml`-compatible Serde and
  error-location surface; or
- a maintained Serde wrapper over `saphyr`/another parser can demonstrate the
  existing JSON and diagnostic contract without assertion changes.

Include `serde_norway`, `noyalib`, and `serde-saphyr` in the next candidate scan
if they remain current, in addition to checking the status of the original
three candidates.

## Acceptance criteria

- Verify current maintenance from crates.io and the canonical source repository.
- Compare `Error::location()` behavior and preserve every existing assertion in
  `crates/zfb-content/tests/error_messages.rs` and
  `crates/zfb-md-wasm/tests/api.rs` unedited.
- Differential-test `from_str::<serde_json::Value>` over the #2750 corpus,
  including anchors/merge keys, non-string keys, scalar edge cases, Unicode,
  and malformed input.
- Pass `cargo check --target wasm32-unknown-unknown -p zfb-md-wasm` and
  `pnpm test:md-wasm` with an isolated `CARGO_TARGET_DIR`.
- Report all four gzip-9 wasm deltas against
  `_temp-resource/dep-audit-baseline/baseline-wasm-size.txt` and pass the size
  guard.
- Report exact transitive-package and license changes and pass
  `cargo deny check` without a new exception.
- If a candidate is demonstrably drop-in, migrate it and add the package-facing
  zfb changelog entry. Otherwise update the decision report with the new
  concrete blocker and keep the existing parser.
```
