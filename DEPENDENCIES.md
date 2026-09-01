# DEPENDENCIES.md — Workspace dependency register

This is the durable register for the Rust and npm dependency audit in #2742. It
records the decisions made in the merged audit work, so a future audit can start
from evidence instead of repeating the same probes. The temporary baseline and
decision artifacts used during the audit were removed by #2754 after the final
comparisons; this document is their durable repository-level synthesis.

The published npm packages' runtime supply-chain surface is intentionally kept
separate in [SECURITY-DEPS.md](./SECURITY-DEPS.md).

## Removed in this epic

### Confirmed-dead Rust declarations (#2744)

The nine declarations below had no call sites after a crate-wide search of
`src/`, `tests/`, and `benches/`. The `zfb` manifest was intentionally not part
of this deletion: its `flate2`, `tar`, `hex`, and `zfb-binfetch` declarations
are used by `crates/zfb/build.rs`.

| Crate | Removed declaration | Evidence | Actual lock-file result |
| --- | --- | --- | --- |
| `zfb-build` | `fs2` normal dependency | No `fs2::` or `FileExt` use; the test lock is hand-rolled in `zfb-test-utils`. | `fs2 0.4.3` left `Cargo.lock`. |
| `zfb-css` | `walkdir` normal dependency | No `walkdir` or `WalkDir` use in the crate. | The `zfb-css` edge disappeared; the package remains through other users. |
| `zfb-css` | `serde` normal dependency | No `serde::` use or `Serialize`/`Deserialize` derive; `serde_json` is a separate live dependency. | The `zfb-css` edge disappeared; `serde` remains widely transitive. |
| `zfb-render` | `anyhow` normal dependency | No use in the crate. | The `zfb-render` edge disappeared; the package remains through other users. |
| `zfb-md-extras` | `anyhow` normal dependency | No use in the crate. | The normal edge disappeared; the package remains. |
| `zfb-md-extras` | `anyhow` dev-dependency | No use in the crate's tests or gated test targets. | The dev edge disappeared; the package remains. |
| `zfb-content` | `anyhow` normal dependency | No use in the crate. | The `zfb-content` edge disappeared; the package remains. |
| `zfb-router` | `anyhow` normal dependency | No use in the crate. | The `zfb-router` edge disappeared; the package remains. |
| `zfb-router` | `tracing-subscriber` dev-dependency | No `tracing_subscriber` use; `tracing-test` is used and was retained. | The `zfb-router` edge disappeared; the package remains. |

The only package record removed by #2744 was `fs2 0.4.3`; the baseline count
was 603 and this branch's corresponding graph was 602. The lockfile also lost
the direct package edges listed above, but no `anyhow`, `serde`, `walkdir`, or
`tracing-subscriber` package could be claimed as removed when another path still
used it. This distinction is the audit rule: an unused direct declaration can be
removed even when the package remains in the lockfile.

### Dead `BuildProgress` and `indicatif` (#2745)

`crates/zfb/src/output.rs` contained an explicitly unwired `BuildProgress`
wrapper, its private `fmt_summary` helper, and only the helper's own tests. The
source comment said it was intended for build progress but was not connected to
the build command. The deletion removed that code, its `indicatif` import and
declaration, and the tests for deleted code. `owo-colors` was not removed: the
remaining output helpers still use it.

The actual lock delta was five package records:

- `indicatif 0.18.6`;
- `console 0.16.4`;
- `unit-prefix 0.5.2`;
- `portable-atomic 1.13.1`; and
- `encode_unicode 1.0.0`, which was left unused when `console` disappeared.

The package count therefore moved from 602 to 597 for this topic's before/after
graph. `unicode-width`, `web-time`, and other shared packages were not claimed
as removals because the lock delta did not remove them.

### Narrow graph cleanups (#2747 and #2748)

* **`futures` umbrella (#2747).** `zfb-server` now has `futures-util` once in
  normal dependencies, and the two imports use `futures_util` directly. The
  lockfile changed only by removing the `"futures"` edge from the `zfb-server`
  package stanza. The `futures` package itself remains because `deno_core`
  pulls it in under the default `embed_v8` graph, so the package count is
  unchanged. In the `zfb --no-default-features` compile graph, both `futures`
  and `futures-executor` disappear; this is a build-no-V8 compile-graph win,
  not a workspace package-count reduction.
* **`reqwest`'s `json` feature in `zfb-build` (#2748).** There are no
  `Response::json()` calls in `zfb-build`, so its speculative feature was
  removed while `blocking` and `rustls-tls` remained. `Cargo.lock` did not
  change, and `serde_urlencoded` did not leave: it is still reached through
  `reqwest` and the workspace's `axum` path.
* **Actual feature ownership correction (#2756/#2757).** The live
  `Response::json()` call is in `crates/zfb/src/render_pipeline.rs`, where it
  parses `/__paths__` responses. The production `zfb` `reqwest` declaration
  therefore owns `json`; #2756 added it there and left it absent from
  `zfb-build`. That correction also changed no lock package. It prevents
  feature unification from hiding an undeclared feature requirement.

Across the merged audit, the baseline's 603 Cargo.lock packages became 597:
the six-package net reduction is exactly `fs2` plus the five #2745 packages.
The #2749 Unicode migration is a one-for-one leaf replacement and does not
change that count.

## Considered and deliberately kept

The governing framing is **used versus unused**, not “free versus costly”. A
crate directly imported by first-party code stays a direct dependency even when
another path also pulls it transitively. Conversely, a genuinely unused direct
declaration is removable even if its package remains in `Cargo.lock`.

### Rust dependencies

* **`hex` — terminal KEEP; do not revisit.** A production-only search finds 21
  `hex::` call sites across `zfb`, `zfb-build`, `zfb-content`, `zfb-css`,
  `zfb-islands`, `zfb-md-ast`, and `zfb-types` (tests add separate coverage).
  It is also required by `crates/zfb/build.rs`: `sha256_hex` calls
  `hex::encode` at line 169, `sha256_hex_file` calls it at line 187, and the
  helpers verify the downloaded esbuild binary. Removing it would require
  hand-rolled encoding while buying no package reduction; keep the maintained
  crate.
* **`reqwest` — KEEP.** The five principal non-optional consumers in the audit
  are `zfb`, `zfb-build`, `zfb-binfetch`, `zfb-server`, and `zfb-test-utils`;
  `zfb-render` also has a required optional edge for its embedded-V8 fetch
  transport. The current lock resolves `reqwest 0.12.28`. It shares `hyper`
  with `axum`, `zfb-binfetch` needs real HTTPS to download from GitHub, and the
  call sites span build, server, test, binary-fetch, and request-time paths, so
  replacing it would be a large refactor with little payoff. This register does
  not assign an undefined “largest contributor” ranking to its graph weight.
  The actionable sliver was the unused `zfb-build` `json` feature, handled in
  #2748; the real production `.json()` consumer is owned by `zfb` after #2756.
* **`walkdir` — KEEP.** It is directly used; zero package-count payoff from
  replacement. It drives route, source, graph, and asset walks across the
  workspace, including symlink and depth behavior that callers test explicitly.
* **`owo-colors` — KEEP.** It is directly used; zero package-count payoff from
  replacement. The CLI output and diagnostics modules still use its stream-aware
  color methods and process-wide test override after `BuildProgress` was deleted.
* **`sourcemap` — KEEP.** It is directly used; zero package-count payoff from
  replacement. `zfb-css` parses inline Source Map v3 data to attribute package
  URLs, and the build path uses the same crate for source-map handling; replacing
  it would discard the maintained parser rather than remove an unused edge.
* **`serde_path_to_error` — KEEP.** It is directly used; zero package-count
  payoff from replacement. `zfb` wraps config deserialization with it so errors
  identify fields such as `framework` and indexed collection paths.
* **`html5ever` + `markup5ever_rcdom` — KEEP as a pair in `zfb-test-utils`.**
  `html5ever` 0.27 parses HTML fragments and serializes them; `RcDom` supplies
  the mutable tree used to sort attributes, normalize nodes, and produce
  canonical test output. The versions must stay compatible with the
  `markup5ever` version they share.
* **`lol_html` — KEEP in its four current crates.** `zfb`, `zfb-build`,
  `zfb-islands`, and `zfb-server` use its selector/token rewriting for live
  HTML injection, island-marker handling, and link/base rewriting. This is not
  the same problem as the `html5ever`/`RcDom` tree normalizer: `lol_html` is a
  streaming rewriter and does not replace a mutable HTML5 tree for test
  canonicalization. These dependencies do not genuinely consolidate; leave
  both roles explicit.
* **`bincode` 2 — KEEP in `zfb-graph`.** This first-party-only direct edge is
  removable in principle, but it serializes the on-disk dependency graph cache
  through the `serde` integration. The wire-format version was bumped during
  the bincode 1→2 migration because the encoding differs, and the existing
  cache invalidation behavior depends on that explicit version.
* **`imagesize` — KEEP in `zfb-md-extras`.** This exclusively ours direct edge
  is removable in principle, but the image-dimensions feature probes raster
  headers without decoding complete image payloads, preserving the warning and
  cache behavior of the plugin.
* **`roxmltree` — KEEP in `zfb-md-extras`.** This exclusively ours direct edge
  is removable in principle, but `imagesize` cannot read SVGs, so the plugin
  uses `roxmltree` to parse SVG `width`/`height` and `viewBox` dimensions while
  skipping files with no determinable intrinsic size.
* **`html-escape` — KEEP in `zfb-content`.** This exclusively ours direct edge
  is removable in principle, but the directive parser decodes HTML character
  references in attribute values before constructing its typed nodes; it is a
  live parser operation, not an unused declaration.

### Rust decisions recorded by the audit

* **Unicode categories (#2749) — MIGRATE.** `unicode_categories 0.1.1` was
  replaced by maintained `unicode-general-category 1.1.0`. The parser needs the
  Unicode general-category `P | S` predicate for directive-name boundaries;
  preserving it with an in-house table would hit the abandon rule. The full
  scalar differential covered 1,112,064 values and found 1,855 differences
  (1,854 new punctuation/symbol boundaries and U+111C9 changing from
  punctuation to Unicode-16 `Mn`). The lock change is one leaf out and one leaf
  in, with no package-count reduction. The package-facing behavior is recorded
  in the `zfb` and `zfb-md-wasm` v2.14.0 lanes; the other three lanes say no
  package-specific changes.
* **`serde_yaml 0.9.34+deprecated` (#2750, #2774) — RETAIN.** Final re-check
  2026-09-01. The released candidates evaluated in #2787 and #2788 each
  diverged from the committed `serde_yaml` baseline in 11 of 18 cases. A
  pinned pre-release re-check of noyalib's post-cutoff `feat/v0.0.29` branch
  in #2808 still found 11 mismatches through its compatibility shim and 9
  through the closest configured direct path. Concrete JSON, resource-limit,
  error-rendering, and diagnostic-location blockers therefore outweigh the
  candidates' maintenance advantage. Nothing in CI forces a migration:
  `deny.toml` has no `serde_yaml` or `unsafe-libyaml` exception or advisory
  entry, and advisories are the security workflow's only deny-on-finding
  section. The maintenance liability is also confined to a tiny current
  surface: one frontmatter `from_str::<serde_json::Value>` call, one `#[from]`
  error conversion, and `Error::location()` consumers in `zfb` and
  `zfb-md-wasm`; `serde_yaml::Value` is not used.

  The standing abandon rule makes a migration terminally KEEP if it needs more
  than 40 rustfmt-formatted non-test production lines, a second corrective
  candidate round, or unsafe/FFI, OS-specific branches, polling, signal
  supervision, or a generated Unicode table. Neither evaluated candidate hit
  those mechanical thresholds: the noyalib compatibility port was 2
  production lines, its configured direct path was 5, and the serde-saphyr
  port was 10, each in one candidate round and without a prohibited
  mechanism. Their semantic incompatibilities decided the result before Phase
  2, so no wasm size, transitive-package, or license delta was measured and no
  migration issue or changelog entry is warranted.
  [#2755](https://github.com/Takazudo/zudo-front-builder/issues/2755) remains
  open as the standing re-evaluation trigger.

  * **`serde_yaml_ng` — no trigger:** `0.10.0` released 2024-05-26
    ([crates.io record](https://crates.io/api/v1/crates/serde_yaml_ng/0.10.0));
    latest canonical commit 2025-09-14
    ([commit](https://github.com/acatton/serde-yaml-ng/commit/3628102977f3ec9e02b95ef32fcec30b3df91390)).
    It matched the tested API and behavior in #2750, but has no post-0.10
    release or ongoing-maintenance evidence, so the maintenance blocker remains.
  * **`serde_yml` — no trigger:** `0.0.13` released 2026-05-27
    ([crates.io record](https://crates.io/api/v1/crates/serde_yml/0.0.13));
    latest canonical commit 2026-05-28
    ([commit](https://github.com/sebastienrousseau/serde_yml/commit/5caeeec0512296f985135502d36cd08e8ffb23d1)).
    The crate is deprecated/unmaintained and its canonical repository is
    archived; [RUSTSEC-2025-0068](https://rustsec.org/advisories/RUSTSEC-2025-0068.html)
    has no patched version, and the shim's error
    locations/merge-key behavior are not the #2750 contract.
  * **`saphyr` — no trigger:** `0.0.12` released 2026-08-18
    ([crates.io record](https://crates.io/api/v1/crates/saphyr/0.0.12)); latest
    canonical commit 2026-08-18
    ([commit](https://github.com/saphyr-rs/saphyr/commit/5c45acd365c711e3d92f0ed4a0ceabe349cde514)).
    It is an active YAML parser/object library, but not a Serde deserializer;
    the released crate has no `from_str::<serde_json::Value>` replacement.
  * **`serde_norway` — no trigger:** `0.9.42` released 2024-12-21
    ([crates.io record](https://crates.io/api/v1/crates/serde_norway/0.9.42));
    latest canonical commit 2024-12-21
    ([commit](https://github.com/cafkafk/serde-norway/commit/1d37c159fc01c269a17ab72d021b271faf29472a)).
    The Serde-shaped fork remains available, but release and repository activity
    provide no evidence of ongoing maintenance at this re-check.
  * **`noyalib` — evaluated; retain:** `0.0.28` remains the newest release
    ([crates.io record](https://crates.io/api/v1/crates/noyalib/0.0.28), checked
    2026-09-01), while canonical pre-release branch `feat/v0.0.29` is 40
    commits ahead of `main` at
    [`697195f`](https://github.com/sebastienrousseau/noyalib/commit/697195f15ffa0477d2de02b19d7d8253819e10c5)
    and [release PR 365](https://github.com/sebastienrousseau/noyalib/pull/365)
    remains open with no `v0.0.29` tag or GitHub Release (checked 2026-09-01).
    The pinned branch improves custom-tag rejection and its configured direct
    path can preserve merge keys and `u64::MAX`, but 11 compatibility-shim and
    9 configured-direct corpus mismatches remain across composite keys, scalar
    resolution, diagnostics, overflow, and alias limits
    ([#2808](https://github.com/Takazudo/zudo-front-builder/issues/2808), checked
    2026-09-01). Publication of a release containing this work is the next
    trigger; the unreleased branch is not migration-eligible.
  * **`serde-saphyr` — evaluated; retain:** the planned `1.1.0` screen was
    superseded by `1.2.0`, released 2026-08-30
    ([1.2.0 crates.io record](https://crates.io/api/v1/crates/serde-saphyr/1.2.0),
    checked 2026-08-30; [1.1.0 record](https://crates.io/api/v1/crates/serde-saphyr/1.1.0),
    checked 2026-08-30). Latest canonical commit on 2026-08-30 is recorded
    here ([commit](https://github.com/bourumir-wyngs/serde-saphyr/commit/45042059e6905e833516f52d958cba4c16e8cedd),
    checked 2026-09-01). Its closest-configured JSON path diverged in 11 of 18
    cases: non-string keys, old-style numbers, built-in and custom tags,
    non-finite and overflowing numbers, alias-expansion limits, all malformed
    error displays, and both protected diagnostic-location pins are concrete
    blockers. Options repaired individual policies but not the contract as a
    whole ([#2788](https://github.com/Takazudo/zudo-front-builder/issues/2788),
    checked 2026-08-31).

  The machine-readable drift baseline is `scripts/yaml-candidate-baseline.json`,
  and `.github/workflows/yaml-candidate-watch.yml` is the weekly lane that
  re-checks it. The lane enumerates **all branches** because the 2026-08-31
  correction on #2755 records that a prior round *"inspected default branches
  and missed `noyalib`'s `feat/v0.0.29` release branch."* A detected
  `CANDIDATE_DRIFT` opens a tracking issue referencing
  [#2755](https://github.com/Takazudo/zudo-front-builder/issues/2755) for triage;
  the watcher does not itself decide that the #2755 trigger has
  fired. The baseline is refreshed only as part of a recorded triage, never
  merely to turn the lane green.

### Candidate desk research (#2785; no-build, checked 2026-08-30)

This is the requested candidate screen, not a migration recommendation. The
actual zfb contract is the direct YAML-frontmatter parse into
`serde_json::Value`, conversion of `serde_yaml::Error` into
`FrontmatterError::Yaml`, and consumption of `location().line()`,
`location().column()`, and (in md-wasm) `location().index()`; `serde_yaml::Value`
is not part of the public surface ([frontmatter.rs](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb-content/src/frontmatter.rs),
checked 2026-08-30; [zfb diagnostics](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb/src/diagnostics.rs),
checked 2026-08-30; [md-wasm diagnostics](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb-md-wasm/src/lib.rs),
checked 2026-08-30). The issue requires these points to be checked against
tagged values, YAML 1.1-vs-1.2 booleans, merge keys, non-string keys, and
error locations, with differential tests still belonging to the decision
sub-issue ([#2785](https://github.com/Takazudo/zudo-front-builder/issues/2785),
checked 2026-08-30). Both candidates below are screened at their newest
requested release and no binding verdict is recorded ([noyalib 0.0.28
metadata](https://crates.io/api/v1/crates/noyalib/0.0.28), checked 2026-08-30;
[serde-saphyr 1.2.0 metadata](https://crates.io/api/v1/crates/serde-saphyr/1.2.0),
checked 2026-08-30).

The planning constraints remain unchanged: the current `deny.toml` has no
`serde_yaml` or `unsafe-libyaml` advisory entries, and the security workflow
makes advisories the only deny-on-finding section while licenses, bans, and
sources remain warn-only. Maintenance evidence is therefore the trigger
screen, not a new CI or advisory verdict ([deny.toml](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/deny.toml),
checked 2026-08-30; [security-audit workflow](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/.github/workflows/security-audit.yml),
checked 2026-08-30; [#2785](https://github.com/Takazudo/zudo-front-builder/issues/2785),
checked 2026-08-30).

#### `noyalib 0.0.28`

* **Release and maintenance evidence.** The 0.0.28 registry record reports
  release creation on 2026-08-24, `yanked: false`, `rust_version: 1.86.0`,
  and `MIT OR Apache-2.0`; its release page is published on 2026-08-24
  ([crates.io version record](https://crates.io/api/v1/crates/noyalib/0.0.28),
  checked 2026-08-30; [GitHub release v0.0.28](https://github.com/sebastienrousseau/noyalib/releases/tag/v0.0.28),
  checked 2026-08-30). The registry history contains 26 non-yanked releases
  from 0.0.1 on 2026-05-10 through 0.0.28 on 2026-08-24, which is release
  cadence evidence rather than a guarantee of future maintenance
  ([crates.io history](https://crates.io/api/v1/crates/noyalib), checked
  2026-08-30). Canonical repository metadata reports `archived: false`,
  `disabled: false`, and a push on 2026-08-25; the checked version response
  contains no `deprecated` field ([repository metadata](https://api.github.com/repos/sebastienrousseau/noyalib),
  checked 2026-08-30; [crates.io version record](https://crates.io/api/v1/crates/noyalib/0.0.28),
  checked 2026-08-30). The immutable release source describes a pure-Rust
  YAML 1.2 implementation with Serde integration and no unsafe in the
  library ([v0.0.28 README](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/README.md),
  checked 2026-08-30).
* **Tagged values.** Native `noyalib::Value` has seven variants, including
  `Tagged`, and the migration guide says custom tags are retained there
  ([value source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/value.rs),
  checked 2026-08-30; [migration guide](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/doc/MIGRATION-FROM-SERDE-YAML.md),
  checked 2026-08-30). zfb requests `serde_json::Value`, not native
  `noyalib::Value`; the v0.0.28 deserializer uses a special fast path only
  for the native type, while `Value::Tagged` recurses through its inner value
  for generic `deserialize_any`. The source therefore predicts that a custom
  tag will not become a JSON tag node in this zfb call, but this is a
  source-derived inference that requires a differential fixture
  ([deserializer source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/de.rs),
  checked 2026-08-30; [Serde value adapter](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/value/serde_impl.rs),
  checked 2026-08-30).
* **Booleans.** The default parser follows YAML 1.2 boolean resolution:
  bare `yes`, `no`, `on`, and `off` remain strings, while the migration guide
  documents an opt-in legacy-booleans setting for YAML 1.1 spellings
  ([configuration source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/de/config.rs),
  checked 2026-08-30; [migration guide](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/doc/MIGRATION-FROM-SERDE-YAML.md),
  checked 2026-08-30). Because the zfb target is the generic
  `serde_json::Value` path, the source predicts strict YAML 1.2 JSON values
  under default configuration; the exact scalar corpus remains a differential
  test obligation ([loader source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/parser/loader.rs),
  checked 2026-08-30).
* **Merge keys.** `MergeKeyPolicy::Auto` is the default; `AsOrdinary` and
  `Error` are available. The v0.0.28 policy and tests say only a plain `<<`
  key is eligible, while quoted `"<<"` and an alias resolving to a string are
  ordinary keys ([policy source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/de/config.rs),
  checked 2026-08-30; [policy tests](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/tests/merge_key_policy.rs),
  checked 2026-08-30; [plain-key tests](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/tests/merge_key_plain_only.rs),
  checked 2026-08-30). The loader has no corresponding explicit `!!merge`
  branch in the release source; unknown tags are represented as `Tagged`, so
  explicit-tag behavior is an uncertainty to test rather than an assumed
  compatibility match ([loader source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/parser/loader.rs),
  checked 2026-08-30).
* **Non-string keys.** The public `Mapping` stores `IndexMap<String, Value>`;
  the loader's `value_to_key_string` converts scalar, tagged, sequence, and
  mapping keys to deterministic strings and reports `KeyCollision` when
  distinct typed keys stringify identically ([mapping source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/value/mapping.rs),
  checked 2026-08-30; [loader source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/parser/loader.rs),
  checked 2026-08-30). Inference for this zfb surface: numeric or composite
  YAML keys that current direct `serde_yaml`→JSON conversion rejects may be
  accepted as JSON object strings by noyalib, subject to collision errors;
  this is a material differential-test risk ([current frontmatter call](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb-content/src/frontmatter.rs),
  checked 2026-08-30).
* **Errors and locations.** v0.0.28 exposes `Error::location()` as an
  `Option<Location>`; parser locations are 1-based for line and column and
  `index()` is a 0-based UTF-8 byte offset, with no location for several
  unlocated error variants. Its structured `Display` prefixes (for example,
  `YAML parse error at ...`) are its own wording, so zfb's verbatim
  `FrontmatterError::Yaml` message would need differential review
  ([error source](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/error.rs),
  checked 2026-08-30; [compatibility surface](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/crates/noyalib/src/compat/serde_yaml.rs),
  checked 2026-08-30; [zfb consumers](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb/src/diagnostics.rs),
  checked 2026-08-30; [md-wasm consumers](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb-md-wasm/src/lib.rs),
  checked 2026-08-30).
* **Abandon-rule pre-assessment.** A direct noyalib port changes the error
  type and parse call in `frontmatter.rs`; the compat port changes those two
  paths to `noyalib::compat::serde_yaml` even though it is not a Cargo package
  alias. Across the four known production callsites (two frontmatter lines and
  two diagnostic consumers), the existing line/column/index method names and
  relative frontmatter offset can remain conceptually the same, so the initial
  production delta is estimated at roughly 2–4 rustfmt lines, below the
  40-line trigger. The
  candidate screen found no required unsafe, FFI, OS-specific branch, polling
  loop, signal supervision, or Unicode table; this is a pre-assessment only,
  and any adapter required by differential tests must be re-counted against
  the standing rule ([compat migration notes](https://github.com/sebastienrousseau/noyalib/blob/0a0c75faefdd2e1ba5ea06d4fe9b372154a99a6e/doc/MIGRATION-FROM-SERDE-YAML.md),
  checked 2026-08-30; [abandon rule](#abandon-rule), checked 2026-08-30).

##### `noyalib 0.0.28` differential evaluation (#2787) — KEEP

This evaluation started from
`b7edbf7428fbc9f7d5cc21fe933e0f278671164c`. It used the committed #2786
corpus and immutable serde_yaml baseline through separate
`noyalib::compat::serde_yaml::from_str::<serde_json::Value>` and
`noyalib::from_str::<serde_json::Value>` adapters, plus temporary production
wiring through each path. The compat and direct observations were identical,
and each differed from the baseline in 11 of 18 cases:

* the default `MergeKeyPolicy::Auto` expanded the plain `<<` mapping instead
  of preserving `<<` as an ordinary JSON key;
* a composite sequence key was accepted as the string `"[a, b]"` instead of
  failing at `1:1:0`; old-style `0123` became the number `123` instead of a
  string, while `0b11` became a string instead of the number `3`;
* a custom tagged value was accepted as the untagged JSON string `"value"`
  instead of producing serde_yaml's located JSON-enum error. Thus native
  `Value::Tagged` retention does not preserve the current zfb
  `serde_json::Value` contract;
* non-finite values still normalized to JSON null, but overflow `1e999` also
  became null instead of the string `"1e999"`; `u64::MAX` lost integer
  precision by becoming an approximate floating-point JSON number, and the
  one-past-`u64::MAX` case was accepted similarly instead of failing;
* malformed-input `Display` text used noyalib wording. For example, the
  Unicode case changed from `did not find expected node content at line 1
  column 8, while parsing a flow node` to `YAML parse error at line 1, column
  8: expected a node but found FlowMappingEnd`. Its location remained
  `1:8:16`, but the EOF flow-sequence pin changed from `2:1:12` to `1:13:12`,
  and the protected md-wasm API assertion consequently observed source line
  2 instead of 3. The indentation error retained `2:9:18` but changed its
  display text;
* the bounded alias-expansion case succeeded and materialized the expanded
  value instead of returning serde_yaml's unlocated `repetition limit
  exceeded` error.

The YAML-boolean case did match: legacy bare `y`, `yes`, `n`, `no`, `on`, and
`off` remained strings while `true` and `FALSE` resolved as booleans. The
Unicode/CRLF/emoji value case, scalar non-string keys, built-in tags, duplicate
keys, and the remaining successful/error cases also matched. Nevertheless,
the merge, tag/JSON, number, resource-limit, error-display, and EOF-location
differences are concrete production-contract blockers. The terminal verdict
is **KEEP `serde_yaml`**; Phase 2 was therefore skipped exactly as required
(no wasm32 or `pnpm test:md-wasm` run, no four-artifact size measurement, no
transitive/license diff, and no `cargo deny` run).

The temporary replacement changed 2 rustfmt-formatted non-test production
lines in `frontmatter.rs`, below the 40-line threshold, and required no unsafe,
FFI, OS branch, polling, signal supervision, or Unicode table. One candidate
round was run; a test-only oracle declaration was corrected once so the
immutable serde_yaml adapter remained available. No behavioral compatibility
adapter or candidate correction was attempted because Phase 1 already decided
the candidate. The release and maintenance records used by this run remain the
immutable 0.0.28 crates.io record (released 2026-08-24) and canonical v0.0.28
repository release, both
checked 2026-08-30 ([crates.io version record](https://crates.io/api/v1/crates/noyalib/0.0.28);
[GitHub release v0.0.28](https://github.com/sebastienrousseau/noyalib/releases/tag/v0.0.28)).
All experimental manifest, lockfile, production source, test-probe, and target
artifacts were restored or removed; the final committed delta is this evidence
only ([#2787](https://github.com/Takazudo/zudo-front-builder/issues/2787),
checked 2026-08-30).

##### `noyalib 0.0.29` pre-release differential evaluation (#2808) — KEEP

The post-#2787 live-source confirmation looked only at canonical default
branches and therefore missed real work after its
`2026-08-30T15:20:51Z` cutoff. The `noyalib` repository's
`feat/v0.0.29` branch was 40 commits ahead of `main` at immutable commit
[`697195f`](https://github.com/sebastienrousseau/noyalib/commit/697195f15ffa0477d2de02b19d7d8253819e10c5)
(`2026-08-30T22:56:51Z`). Its changelog and source include located typed
deserialization errors, retained streaming messages, tagged-scalar handling,
plain-scalar configuration, and number changes relevant to the earlier
blockers. [Release PR 365](https://github.com/sebastienrousseau/noyalib/pull/365)
was open and green at both the start and end of this evaluation, its manifest
said `0.0.29`, and crates.io still exposed only
[`0.0.28`](https://crates.io/api/v1/crates/noyalib/0.0.28); no `v0.0.29` tag or
GitHub Release existed (all checked 2026-09-01). The branch is maintenance and
pre-release evaluation evidence, not a shipped migration target.

This evaluation started from zfb commit
`ec6e9a14d3e603e2ec21166bfedae96538c7ecfd` and pinned every candidate build to
`697195f15ffa0477d2de02b19d7d8253819e10c5`. Phase 1 compared the immutable
18-case serde_yaml baseline with two paths:

* `compat-default` used
  `noyalib::compat::serde_yaml::from_str::<serde_json::Value>`; and
* `direct-configured` used `noyalib::from_str_with_config` with
  `MergeKeyPolicy::AsOrdinary` and the `lossless-u64` feature plus
  `lossless_u64_integers(true)`, the closest documented configuration for the
  two configurable baseline differences.

The new `plain_scalar_strings` flag was deliberately not enabled: pinned source
confines it to explicitly typed `String`/`char` targets, while zfb's
`serde_json::Value` path uses `deserialize_any`; it cannot repair the value
rows below. Lowering noyalib's alias budgets would likewise manufacture a
different budget error rather than serde_yaml's exact unlocated `repetition
limit exceeded` contract, so it is not a viable compatibility setting.

The complete named matrix (`match` means an exactly equal structured
observation, including error text and location) was:

| Corpus case | compat-default | direct-configured |
| --- | --- | --- |
| `anchors-and-aliases` | match | match |
| `merge-key-is-an-ordinary-json-key` | mismatch | match |
| `non-string-scalar-keys` | match | match |
| `non-string-composite-key` | mismatch | mismatch |
| `yaml-11-boolean-spellings` | match | match |
| `octals-sexagesimals-and-numbers` | mismatch | mismatch |
| `null-and-date-scalars` | match | match |
| `unicode-crlf-and-emoji` | match | match |
| `malformed-unicode-location` | mismatch | mismatch |
| `malformed-flow-sequence-at-eof` | mismatch | mismatch |
| `malformed-indentation` | mismatch | mismatch |
| `built-in-explicit-tags` | match | match |
| `custom-explicit-tag` | mismatch | mismatch |
| `duplicate-map-keys-last-wins` | match | match |
| `non-finite-and-overflowing-numbers` | mismatch | mismatch |
| `integer-boundaries` | mismatch | match |
| `integer-overflow` | mismatch | mismatch |
| `alias-anchor-repetition-limit` | mismatch | mismatch |

The compatibility shim therefore remained at **11/18 mismatches**. The
configured direct path improved to **9/18 mismatches**, but still failed the
contract:

* composite sequence keys were accepted as the string `"[a, b]"` instead of
  failing at `1:1:0`; `0123` became numeric `123` rather than a string and
  `0b11` remained a string rather than numeric `3`;
* malformed Unicode kept the correct `1:8:16` location but retained noyalib's
  different `YAML parse error ... FlowMappingEnd` text. The EOF flow-sequence
  error remained `1:13:12` instead of `2:1:12`, and indentation retained
  `2:9:18` with different wording;
* v0.0.29 improved the custom-tag case from #2787's silently accepted,
  untagged JSON string to an error, but it reported
  `deserialization error at line 1, column 16 ...` at `1:16:15` instead of
  serde_yaml's text and `1:8:7` location;
* `1e999` still became JSON null instead of the string `"1e999"`;
  configured `u64::MAX` now matched exactly, but one-past-`u64::MAX` became the
  string `"18446744073709551616"` instead of a located error; and
* the bounded alias-expansion case still materialized the expanded value
  instead of returning the unlocated `repetition limit exceeded` error.

The protected `zfb-content` error-quality assertion passed unchanged for both
production wirings because both still include line/column context. The exact
md-wasm render API assertion failed unchanged for both paths: it observed
source line 2 instead of the protected line 3. The parse-to-AST invalid-YAML
API case and the standalone UTF-16 frontmatter diagnostic test passed. No
protected assertion, corpus case, or baseline record was edited.

The compatibility wiring changed 2 rustfmt-formatted non-test production lines
and the configured direct wiring changed 5, both below the 40-line abandon
threshold. One candidate round used only documented configuration; neither
path required unsafe/FFI, OS branches, polling, signal supervision, or a
generated Unicode table. Because neither path fully matched Phase 1, the
terminal verdict is **KEEP `serde_yaml`**. Phase 2 was skipped as required: no
wasm32 check, full `pnpm test:md-wasm`, four-artifact size measurement,
transitive/license diff, or `cargo deny` run was warranted.

The single isolated target consumed 2.0 GiB and was removed after the run.
`Cargo.toml`, `Cargo.lock`, production code, the corpus/baseline, protected
assertions, generated artifacts, `shipped-sizes.json`, and every changelog were
restored byte-for-byte. The next trigger is publication of a release containing
the evaluated v0.0.29 work; the standing issue remains open. This KEEP has no
package-facing effect and warrants no migration or changelog entry.

#### `serde-saphyr 1.2.0`

* **Release and maintenance evidence.** The 1.2.0 registry record reports
  creation on 2026-08-30, `yanked: false`, `rust_version: 1.89`, and
  `MIT OR Apache-2.0`; it supersedes the planning screen's 1.1.0, which was
  released on 2026-08-15 ([1.2.0 crates.io record](https://crates.io/api/v1/crates/serde-saphyr/1.2.0),
  checked 2026-08-30; [1.1.0 record](https://crates.io/api/v1/crates/serde-saphyr/1.1.0),
  checked 2026-08-30). The stable release sequence is 1.0.0 on 2026-07-31,
  1.0.1 on 2026-08-05, 1.1.0 on 2026-08-15, and 1.2.0 on 2026-08-30,
  which is cadence evidence rather than a guarantee of future maintenance
  ([crates.io history](https://crates.io/api/v1/crates/serde-saphyr), checked
  2026-08-30). Canonical repository metadata reports `archived: false`,
  `disabled: false`, and a push on 2026-08-30; the checked version response
  contains no `deprecated` field, and the 1.2.0 release is published on that
  date ([repository metadata](https://api.github.com/repos/bourumir-wyngs/serde-saphyr),
  checked 2026-08-30; [1.2.0 crates.io record](https://crates.io/api/v1/crates/serde-saphyr/1.2.0),
  checked 2026-08-30; [1.2.0 release](https://github.com/bourumir-wyngs/serde-saphyr/releases/tag/1.2.0),
  checked 2026-08-30). The release README describes a strongly typed,
  fuzz-tested deserializer and says the library build denies unsafe, while
  explicitly leaving transitive dependencies to a separate audit
  ([v1.2.0 README](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/README.md),
  checked 2026-08-30).
* **Tagged values.** serde-saphyr has no abstract `Value` DOM; it supports
  direct deserialization to `serde_json::Value`, exposes `Tagged<T>` for a
  caller that explicitly wants a tag, and defaults to permissive handling of
  unsupported tags ([README](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/README.md),
  checked 2026-08-30; [tag options/tests](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/tests/unsupported_tags.rs),
  checked 2026-08-30). Its generic `deserialize_any` path discards YAML tags
  for ordinary untyped visitors, so the zfb JSON target is expected to lose
  tag identity unless a typed wrapper is introduced; that expectation must be
  verified against the fixture corpus ([deserializer source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/deserializer.rs),
  checked 2026-08-30).
* **Booleans.** `strict_booleans` defaults to false. In the typeless
  `deserialize_any` path used by a JSON-like target, the release parser
  recognizes YAML 1.1 spellings such as `y`, `yes`, `on`, `n`, `no`, and
  `off` as booleans; the README warns that this can be surprising for
  `serde_json::Value`, and `strict_booleans: true` is the opt-in YAML 1.2
  behavior ([options source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/options.rs),
  checked 2026-08-30; [scalar resolver](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/parse_scalars.rs),
  checked 2026-08-30; [README warning](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/README.md),
  checked 2026-08-30).
* **Merge keys.** The default `MergeKeyPolicy::Merge` expands both plain
  `<<` and an explicitly resolved `!!merge` key; `AsOrdinary` and `Error` are
  available alternatives. The release tests exercise merge expansion into
  `serde_json::Value` ([options source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/options.rs),
  checked 2026-08-30; [merge-key source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/key_nodes.rs),
  checked 2026-08-30; [JSON-value test](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/tests/serde_yaml/test_from_str_value.rs),
  checked 2026-08-30).
* **Non-string keys.** serde-saphyr records typed key fingerprints for
  duplicate detection and its string-key deserializer rejects integer/core-tag
  keys that cannot be parsed as `String`; the historical regression test
  expects a composite key into `HashMap<String, String>` to fail
  ([key-node source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/key_nodes.rs),
  checked 2026-08-30; [deserializer source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/deserializer.rs),
  checked 2026-08-30; [historical regression test](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/tests/serde_yaml/test_historical_failures.rs),
  checked 2026-08-30). This differs from noyalib's explicit stringification
  risk and needs a direct `serde_json::Value` differential case; no DOM path
  exists to preserve typed keys ([README](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/README.md),
  checked 2026-08-30).
* **Errors and locations.** `serde_saphyr::Error::location()` returns an
  `Option<Location>`, with line and column exposed as 1-based `u64` character
  positions. `Location::span().byte_offset()` is an optional byte offset for
  string input; there is no serde_yaml-shaped `index()` method, and alias
  errors can carry multiple locations. Display text includes structured
  messages/snippets and therefore differs from zfb's current verbatim error
  embedding ([location source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/location.rs),
  checked 2026-08-30; [span source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/span.rs),
  checked 2026-08-30; [error source](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/src/de/error.rs),
  checked 2026-08-30; [location tests](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/tests/location.rs),
  checked 2026-08-30). The zfb diagnostic path would need `u64`→`usize`
  conversion and an md-wasm replacement for `index()` using the optional
  byte offset, with explicit fallback behavior for `None` ([zfb diagnostics](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb/src/diagnostics.rs),
  checked 2026-08-30; [md-wasm diagnostics](https://github.com/Takazudo/zudo-front-builder/blob/146eb937b5ae33e8742a07207417ab19ef1b0b2e/crates/zfb-md-wasm/src/lib.rs),
  checked 2026-08-30).
* **Abandon-rule pre-assessment.** A direct serde-saphyr port changes the
  frontmatter error type and parse call, adds the diagnostic integer casts,
  and adapts md-wasm from `location().index()` to
  `location().span().byte_offset()`; the initial estimate is roughly 7–10
  rustfmt lines across the four known production callsites, below the 40-line
  trigger. The candidate screen found no
  required unsafe, FFI, OS-specific branch, polling loop, signal supervision,
  or Unicode table; the README's direct-library unsafe statement does not
  replace the required transitive audit. Any adapter required by differential
  tests must be re-counted ([README](https://github.com/bourumir-wyngs/serde-saphyr/blob/45042059e6905e833516f52d958cba4c16e8cedd/README.md),
  checked 2026-08-30; [abandon rule](#abandon-rule), checked 2026-08-30).

##### `serde-saphyr 1.2.0` differential evaluation (#2788) — KEEP

This evaluation deliberately used **serde-saphyr 1.2.0**, superseding the
1.1.0 named in #2774, and started from
`a52971ba678b00ba3bf616d1117a49b1018d953e`. Phase 1 exercised the committed
#2786 corpus through the default `from_str::<serde_json::Value>` path and a
closest-contract option set (`LastWins`, merge keys as ordinary keys, strict
booleans, rejected unsupported tags, and permitted non-finite typeless
floats). It also compiled a temporary direct production port and exercised a
typed anchor/alias struct. serde-saphyr's streaming typed path successfully
resolved the corpus anchor and alias to equal typed values. Its lack of a
native `Value` DOM does not prevent direct JSON deserialization in general,
but it could not produce the current JSON result for every corpus case.

The closest-configured JSON path differed from the immutable serde_yaml
baseline in **11 of 18 cases**, spanning the required categories:

* anchors/aliases, null/date scalars, Unicode/CRLF/emoji values, integer
  boundaries, configured merge-as-ordinary behavior, strict booleans, and
  configured duplicate-key last-wins behavior matched;
* scalar non-string keys failed on the null key instead of producing the
  baseline JSON object, while the composite-key error had different text;
* old-style `0123` became `123.0` instead of remaining the string `"0123"`;
* all three malformed inputs changed `Display` rendering to serde-saphyr's
  annotated snippet format. The Unicode error moved from `1:8:16` to line 1,
  column 7 with no byte offset; the EOF flow sequence moved from `2:1:12` to
  line 1, column 8 with no byte offset; and the indentation error retained
  line 2, column 9 but exposed no byte offset;
* built-in explicit tags failed where serde_yaml produced JSON, while a custom
  tag was silently discarded by default. Rejecting unsupported tags restored
  an error but not serde_yaml's text or `1:8:7` location;
* permitting non-finite typeless floats produced the strings `".nan"`,
  `".inf"`, and `"-.inf"`, and normalized `1e999` to `".inf"`, rather than
  serde_yaml's JSON nulls plus string `"1e999"`; one-past-`u64::MAX` was
  accepted as an approximate floating-point number instead of failing at
  `1:11:10`;
* the bounded alias-expansion case succeeded and materialized the expanded
  value instead of returning serde_yaml's unlocated `repetition limit
  exceeded` error.

The default option set diverged further: it expanded merge keys, resolved YAML
1.1 `y`/`yes`/`n`/`no`/`on`/`off` spellings as booleans, rejected duplicate
keys, rejected non-finite typeless floats, and accepted the custom tag after
discarding its identity. Thus options can repair individual policies but do
not repair the JSON, tag, number, resource-limit, error-rendering, or location
contract as a whole.

The temporary direct port changed **10** rustfmt-formatted non-test production
lines across the four production call sites: two in `frontmatter.rs`, one in
`diagnostics.rs`, and seven in `zfb-md-wasm/src/lib.rs`. This is below the
40-line abandon threshold and required no unsafe, FFI, OS-specific branch,
polling, signal supervision, or Unicode table. The native candidate port
compiled for `zfb-content` and `zfb-md-wasm`; the broader `zfb` check reached
its pre-existing build-script prerequisite and stopped because the frozen
pnpm packages were not installed. Installing them was unnecessary after the
semantic blocker. The protected `zfb-content/tests/error_messages.rs` suite
still passed, but the untouched md-wasm API pin observed source line 2 instead
of 3, and the untouched `parse_to_ast.rs` Unicode pin observed column 7 instead
of UTF-16 column 9. One candidate round was run and no corrective production
round was attempted.

These are concrete production-contract blockers, so the terminal verdict is
**KEEP `serde_yaml`**. Phase 2 was skipped exactly as required: no wasm32
check, `pnpm test:md-wasm`, four-artifact size measurement, transitive/license
diff, or `cargo deny` run. All temporary manifest, lockfile, production,
test-adapter, and generated changes were restored byte-identically to the
starting revision, and all isolated Cargo target directories were removed.
The final committed delta is this evidence only ([#2788](https://github.com/Takazudo/zudo-front-builder/issues/2788),
checked 2026-08-31).

This desk research does not select either candidate. The release substitution
to serde-saphyr 1.2.0, the JSON/tag/boolean/merge/key/location risks, and the
line-count pre-assessments are inputs for the differential-test decision; no
dependency, build, production-code, or test-fixture change is authorized by
this section ([#2785](https://github.com/Takazudo/zudo-front-builder/issues/2785),
checked 2026-08-30).

* **Platform/process utilities (#2751) — KEEP.** `local-ip-address` enumerates
  every non-loopback IPv4 interface for bind-all ready URLs; a UDP-connect
  shortcut would select only one egress address, and the repository has no
  Windows Rust test lane to prove equivalent interface enumeration. `wait-timeout`
  provides the cross-platform `ChildExt::wait_timeout` path that kills and
  reaps a timed-out child; replacing it with polling is both a semantic change
  and an abandon-rule trigger. `npm-run-all2` supervises the two long-running
  docs processes in `dev` and `dev:network`; a replacement must forward signals,
  propagate exit status, and clean up both children. No such replacement met
  the required contrary gate.
* **JavaScript checker (#2752) — recommendation, not a new dependency.** Do
  not make a JS checker required in this audit. If revisited, pilot a pinned
  **Knip** release in report-only mode rather than depcheck: it better models
  pnpm projects, entry points, exports, and dependency categories. Reconcile
  every report against the generated docs bundle, the optional peer keep-list
  below, and publishable package contracts; promote only after two
  clean pilots and reviewed configuration. The existing cargo-machete guard is
  the lower-noise Rust check.

### npm keep-list from `docs/CLAUDE.md`

The docs site deliberately installs these **optional peer feature packages** of
`@takazudo/zudo-doc`: `diff`, `katex`, `preact`, `zod`, `@takazudo/zdtp`, and
`@takazudo/zudo-doc-history-server`. They satisfy opt-in package features and
the history server's docs development process; they are not dead because their
imports are not all in `docs/src`.

## walkdir consolidation is an explicit non-goal

There are approximately 75 `walkdir` call sites and no package-count payoff
from replacing them with `ignore`: `walkdir` is already directly used in the
route, graph, build, CSS, and island paths. `ignore`'s gitignore awareness is a
semantic change to directory walks, not a drop-in implementation detail; it
would change which files are visited in paths that intentionally preserve
different symlink, depth, and generated-directory rules. Do not start a future
audit by treating this as an open consolidation task.

## Duplicate-version note

The duplicate-version capture taken at the audit's parent commit recorded these
version families:

| Family | Baseline ownership/control analysis |
| --- | --- |
| `ahash` 0.7 / 0.8 | 0.7 is under `lightningcss`'s `parcel_sourcemap`/`rkyv` branch. 0.8 is shared by the `lightningcss`/`minify-html` side and the SWC `swc_atoms` side. There is no first-party `ahash` declaration to collapse; upstream graph ownership controls these versions. |
| `bincode` 1.3 / 2.0 | 1.3 is pulled by `deno_core` and `syntect`; 2.0 is the deliberate first-party `zfb-graph` cache codec. The owned 2.0 edge cannot be replaced with the old external codec without undoing the maintained-format migration. |
| `compact_str` 0.7 / 0.8 / 0.9 | 0.7 comes from SWC `swc_ecma_codegen`, 0.8 from `garde`, and 0.9 from Oxc (`oxc_resolver`/`oxc_span`). None is a first-party direct declaration; aligning them requires upstream upgrades, not a safe manifest edit here. |
| `cssparser` 0.33 / 0.36 | 0.33 is dictated by `lightningcss` and its selector/sourcemap stack; 0.36 is used by `lol_html`. Both are external parser owners with live APIs. |
| `getrandom` 0.2 / 0.3 / 0.4 | 0.2 comes through `ahash`/`ring`/Rustls, 0.3 is the optional first-party `zfb-render` Web Crypto edge (and also appears under `ahash` 0.8), and 0.4 comes from `tempfile`. The first-party 0.3 edge is required for OS CSPRNG semantics; the other versions are transitive. |
| `hashbrown` 0.12 / 0.14 / 0.16 / 0.17 | 0.12 is in the `lightningcss`/`rkyv` branch; 0.14 is under `ahash`/SWC and `lightningcss`; 0.16 is used by Oxc's allocator/data structures; 0.17 is reached through `indexmap` across `deno_core`, `lightningcss`, Oxc, and other upstreams. No first-party `hashbrown` declaration owns a version. |
| `base64-simd` 0.7 / 0.8 | 0.7 comes from `lightningcss`'s `parcel_sourcemap`; 0.8 comes from the first-party `sourcemap` path as well as Oxc/SWC sourcemap consumers. The direct `sourcemap` use is live, and its transitive version choice is not ours to pin independently. |

The only first-party ownership choices in this list are the deliberate
`bincode 2` cache codec and `getrandom 0.3` Web Crypto edge, both of which are
live and documented above. `swc_core`, `deno_core`, `lightningcss`, Oxc,
`garde`, `lol_html`, Rustls, and `tempfile` dictate the remaining versions.
There is no currently actionable duplicate for us to fix and no safe duplicate
collapse to perform in this register.

## Deliberate duplication

* **Wrangler.** The root `package.json` and `docs/package.json` each pin
  `wrangler` to exactly `4.85.0`. This is intentional: root scripts and
  showcase/deploy workflows use the root tool, while docs commands use the docs
  importer. `docs/scripts/check-wrangler-pin.mjs` checks both manifests and
  both lockfile importers, so the two pins cannot drift silently.
* **`html-validate`.** The root pins `10.17.0` exactly for root/showcase HTML
  validation, while docs declares `^10.0.0` for its `check:html` script. The
  current lock resolves the docs declaration to `10.17.0`, but the ranges
  express different ownership boundaries. Recommendation: yes, add an
  equivalent pin-guard in a follow-up issue for reproducible root/docs
  validation; do not implement that guard in this documentation issue.

## Abandon rule

This is the standing policy for any future dependency replacement. The source
lesson is
`.claude/skills/l-lessons-client-bundling/SKILL.md`.

Abandon the replacement immediately when any one of these triggers fires:

1. The replacement exceeds **40 lines of rustfmt- or prettier-formatted,
   non-test production code**, counted across every touched file. Compressed
   one-liners do not evade the limit.
2. A **second corrective round** is needed on the same dependency.
3. The replacement requires **`unsafe`, FFI, OS-specific branches, polling
   loops, signal supervision, or Unicode tables**. This is an immediate keep,
   regardless of line count.

On a trigger, restore the dependency, delete the replacement, and record a
terminal **`KEEP`** verdict. A terminal KEEP is a completed sub-task, not a
failure; downstream auto-review must not reopen removal of that dependency.

## npm-workspace inventory

The #2746 inventory covered 14 package manifests: the workspace root,
`docs/`, every `packages/*` manifest, every current `crates/*/npm` manifest, and
`tests/md-wasm-browser-smoke/package.json` (the root plus 13 pnpm projects).
The reserved `examples/*` workspace glob currently has no package manifest.
Test-fixture `package.json` files under crate fixture trees are fixtures, not
workspace package importers.

| Manifest | Package and declarations reviewed | Result |
| --- | --- | --- |
| `package.json` | Private root; six dev tools: `@playwright/test`, `html-validate`, `lefthook`, `prettier`, `vitest`, `wrangler`. | Clean: Playwright, HTML validation, hooks, formatting, tests, and Wrangler workflows/scripts each consume the declared tool. |
| `docs/package.json` | Private docs site; zudo-doc stack, the intentional peer keep-list, TypeScript/types, `html-validate`, `npm-run-all2`, `vitest`, and Wrangler. | Clean after #2746 removed `pagefind`, `remark-directive`, and redundant `gray-matter`; #2825 removed the stale runtime-import keep-list, and zudo-doc 5.14.0's removal of the `gray-matter`/`js-yaml` chain retired the override in #2823. |
| `packages/create-zfb/package.json` | Publishable scaffold with `@takazudo/zfb` dependency and Vitest dev dependency. | Clean: the CLI resolves and spawns the zfb package; tests consume Vitest. |
| `packages/zfb/package.json` | Publishable SDK with five optional platform packages, React peer, and build/test type tooling. | Clean: optional carriers and peer/dev fixtures are part of the package contract. |
| `packages/zfb-runtime/package.json` | Publishable runtime with `hono` dependency, zfb/React peers, and dev fixtures. | Clean: Hono is the runtime router; peer and dev declarations support the published API/tests. |
| `packages/zfb-adapter-cloudflare/package.json` | Publishable adapter with no runtime `dependencies`; Node types, TypeScript, and Vitest are dev-only. | Clean: shipped CLI uses Node built-ins and the project-local worker wrapper. |
| `packages/zfb-darwin-arm64/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-darwin-x64/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-linux-arm64-gnu/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-linux-x64-gnu/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-win32-x64-msvc/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `crates/zfb-islands/npm/package.json` | Private islands runtime package with zfb dependency and test/type tooling. | Clean: the runtime package and its Vitest fixture use the declarations. |
| `crates/zfb-md-wasm/npm/package.json` | Publishable md-wasm package; `@types/mdast`, `mdast-util-directive`, and `mdast-util-mdx` are dependencies, with parser/build tools dev-only. | Clean: published declarations re-export mdast/directive/MDX types and registries, so consumers need these dependencies. |
| `tests/md-wasm-browser-smoke/package.json` | Private browser smoke package with the local packed md-wasm artifact. | Clean: the browser smoke imports the packaged artifact. |

No declaration with no reference was found in the retained inventory. The three
dead docs declarations were removed and documented above. **No publishable
package's `dependencies` field was modified by #2746 or this register**; those
runtime fields remain the separate supply-chain surface audited in
[SECURITY-DEPS.md](./SECURITY-DEPS.md).
