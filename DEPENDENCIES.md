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
* **`serde_yaml 0.9.34+deprecated` (#2750, #2774) — MIGRATED.** Final re-check
  2026-09-02 in
  [#2851](https://github.com/Takazudo/zudo-front-builder/issues/2851). The
  standing #2755 trigger fired and resolved in favour of migrating: the
  released lockstep `noyalib 0.0.30` / `noyalib-serde-yaml 0.0.30` pair matched
  the immutable baseline **18/18** on both the zero-line package-alias and
  two-line direct paths, left every protected assertion unedited, and passed
  Phase 2 green with a net-zero package delta, no new `cargo deny` exception,
  and every wasm artifact under its ceiling. The migration landed 2026-09-03 in
  [#2852](https://github.com/Takazudo/zudo-front-builder/issues/2852) via the
  zero-production-line package alias: root `Cargo.toml` now pins `serde_yaml
  = { package = "noyalib-serde-yaml", version = "=0.0.30" }`, so every existing
  `serde_yaml::` reference in `zfb-content`, `zfb`, and `zfb-md-wasm` compiles
  unchanged. The two authorized sha256 checksums —
  `23d48ffd97a6485043e2d05849188ba375a990a856b3607ce55e585a36ecfbb1`
  (`noyalib 0.0.30`) and
  `d3b783463958e25c83a0aa93aa6adc9d632c85a1ef0aff4d81029871853631c7`
  (`noyalib-serde-yaml 0.0.30`) — matched `Cargo.lock` byte-for-byte.
  `Cargo.lock` moved −`serde_yaml` −`unsafe-libyaml` +`noyalib`
  +`noyalib-serde-yaml`, net zero at 597 packages. The `render-only` wasm
  artifact's measured headroom on the migration build was 6,583 B under its
  1,100,000 B gzip-9 ceiling, matching the #2851 prediction exactly; the other
  three artifacts cleared their ceilings with more room.
  [#2755](https://github.com/Takazudo/zudo-front-builder/issues/2755) remains
  open — it is the standing trigger for continued upstream YAML-candidate
  watching, and its future is the owner's call, not something this migration
  closes. (Superseded 2026-09-03: #2755 was closed with a terminal comment
  once the migration had merged; the watch's protocol now lives in the "YAML
  candidate watch" lane paragraph below.)

  **Re-evaluated 2026-09-03 at `0.0.31` in
  [#2873](https://github.com/Takazudo/zudo-front-builder/issues/2873) —
  MIGRATE.** The adopted pair's next lockstep release, `noyalib 0.0.31` /
  `noyalib-serde-yaml 0.0.31` (sha256
  `6c34297b0e8a3fc5a5245f7ea28e50fc96d290b28e3d0d0da8e7c14235ec33b0` and
  `8eaae0a5d646674f179b3ae62539049568ffa065aaadb89a7135ef5e1c1c274e`), matched
  the immutable baseline **18/18** on both paths, left every protected
  assertion unedited, and passed Phase 2 green: 219/219 md-wasm tests, a
  two-entry-only lock delta at an unchanged 597 packages, no license-category
  change, and `cargo deny check` clean with no new exception. The bump does
  move measured wasm bytes — small deltas in both directions, −460 B to
  +58 B across the sixteen manifest fields, with every artifact still under
  its ceiling and `render-only` gaining 176 B of headroom — so the pin bump
  must refresh the size manifest rather than assume a no-op. The pin bump is
  tracked separately as
  [#2875](https://github.com/Takazudo/zudo-front-builder/issues/2875). **Next
  trigger:** the next `version-published`, `tag-added`, or `release-added`
  delta on a lockstep `noyalib` + `noyalib-serde-yaml` release beyond 0.0.31,
  or a yank of either adopted version — run the evaluation protocol against
  it and never refresh the detector baseline over it.

  **The 0.0.31 bump landed 2026-09-03 in the pin-bump topic
  ([#2875](https://github.com/Takazudo/zudo-front-builder/issues/2875)),
  refs [#2870](https://github.com/Takazudo/zudo-front-builder/issues/2870).**
  Root `Cargo.toml` now pins `serde_yaml = { package = "noyalib-serde-yaml",
  version = "=0.0.31" }`; zero production `.rs` lines changed under the
  abandon rule, so every `serde_yaml::` call site in `zfb-content`, `zfb`,
  and `zfb-md-wasm` still compiles unchanged. `Cargo.lock` moved exactly the
  two adopted entries — `noyalib` and `noyalib-serde-yaml` — from `0.0.30` to
  `0.0.31`, checksums equal to the authorized sha256 pair
  (`6c34297b0e8a3fc5a5245f7ea28e50fc96d290b28e3d0d0da8e7c14235ec33b0` and
  `8eaae0a5d646674f179b3ae62539049568ffa065aaadb89a7135ef5e1c1c274e`), with
  package count unchanged at 597. Re-confirmed on crates.io immediately
  before the bump: both versions still published, un-yanked, checksums
  matching. All named checks passed: 1072/1072 `zfb-content` tests (harness
  18/18 + `error_messages`), 57/57 protected md-wasm tests (`api.rs` +
  `parse_to_ast.rs`), 12/12 `zfb` diagnostics unit tests
  (`--no-default-features`), the `wasm32-unknown-unknown` target check,
  `cargo deny check` clean, 219/219 `pnpm test:md-wasm`, and
  `pnpm format:check`. The Mac build's measured wasm bytes differed from
  this repository's committed manifest for reasons unrelated to the pin
  (platform-specific `wasm-opt`/`wasm-bindgen` codegen, not a semantic
  regression — the eval topic's Mac-measured deltas do not reproduce
  byte-for-byte on CI), so `crates/zfb-md-wasm/shipped-sizes.json` and its
  eight synced doc tables were provisionally refreshed from this Mac build
  (`measuredOnVersion` stays `2.14.3`); every artifact stayed comfortably
  under its ceiling. **Those Mac bytes are not the final manifest.** The
  `wasm-md (default)` job asserts equality against its own ubuntu build — the
  two gzip columns (`gzip9`, `glueGzip9`) banded within a 64-byte tolerance
  (#2878), the remaining columns (`finalWasm`, `glue`) byte-exact — and the
  #2873 section below records that the CI-versus-local gap is
  up to 5,982 B at the same pin — so before this PR merges the manifest must
  be aligned from the PR's own CI build summary and the doc tables re-synced
  via `assert-md-wasm-size-docs.mjs --fix`, exactly as `a6509185` did for
  0.0.30. That alignment was then performed in the epic's root PR from health
  run 33765674744's `wasm-md (default)` build summary (ubuntu): `finalWasm` /
  `gzip9` root 3,394,144 / 1,514,540, highlight 1,539,186 / 817,922, render
  2,189,671 / 1,088,857, parse 693,479 / 281,395 (glue fields unchanged),
  so versus the committed 0.0.30 manifest the pin moved gzip-9 by −884 /
  +741 / +992 / −364 B and `render-only` keeps 11,143 B of headroom under its
  1,100,000 B ceiling. The harness label pair (`CURRENT_ADAPTER_NAME` and the fixture's
  `adapter` field) was relabelled to `0.0.31` in lockstep; the corpus and the
  18 baseline cases are untouched. **Next trigger:** unchanged from above —
  the next lockstep release beyond 0.0.31, or a yank of either adopted
  version.

  **Re-evaluated 2026-09-13 at `0.0.43` in
  [#2988](https://github.com/Takazudo/zudo-front-builder/issues/2988) —
  MIGRATE.** The adopted pair's current lockstep release, eleven versions
  ahead of the committed pin — `noyalib 0.0.43` / `noyalib-serde-yaml 0.0.43`
  (sha256 `55f03682003f288e23f94d8f6222de0dcf15c77329b5e42b2f37421243c1d373`
  and `20846fcf3ba8bf676a2d85708086f7ab6b45c40eb3682a956ffccd7199ded7b2`) —
  matched the immutable baseline **18/18** on the package-alias path, left
  every protected assertion unedited, and passed Phase 2 green: 219/219
  md-wasm tests, a two-entry-only lock delta at an unchanged 597 packages, no
  license-category change, and `cargo deny check` clean with no new exception.
  The bump moves measured wasm bytes, and by an order of magnitude more than
  the 0.0.31 step did: +6,075 / +6,844 / +6,647 B raw and +2,271 / +2,653 /
  +2,034 B gzipped on root, render-only and parse-only, with highlight-only
  alone shrinking by 48 / 37 B. Every artifact still clears its ceiling, but
  `render-only` gzip-9 now clears its 1,100,000 B ceiling by only 4,106 B
  locally (11,142 B on CI's committed numbers), so the pin bump must refresh
  the size manifest from CI's own build summary and should record that
  `render-only` is the artifact nearest its budget. **Next trigger:** the next
  `version-published`, `tag-added`, or `release-added` delta on a lockstep
  `noyalib` + `noyalib-serde-yaml` release beyond 0.0.43, or a yank of either
  adopted version — run the evaluation protocol against it and never refresh
  the detector baseline over it.

  **The 0.0.43 bump landed 2026-09-14 in the pin-bump topic
  ([#2989](https://github.com/Takazudo/zudo-front-builder/issues/2989)),
  refs [#2988](https://github.com/Takazudo/zudo-front-builder/issues/2988).**
  Root `Cargo.toml` now pins `serde_yaml = { package = "noyalib-serde-yaml",
  version = "=0.0.43" }`; zero production `.rs` lines changed under the
  abandon rule, so every `serde_yaml::` call site in `zfb-content`, `zfb`,
  and `zfb-md-wasm` still compiles unchanged. `Cargo.lock` moved exactly the
  two adopted entries — `noyalib` and `noyalib-serde-yaml` — from `0.0.31` to
  `0.0.43`, checksums equal to the authorized sha256 pair
  (`55f03682003f288e23f94d8f6222de0dcf15c77329b5e42b2f37421243c1d373` and
  `20846fcf3ba8bf676a2d85708086f7ab6b45c40eb3682a956ffccd7199ded7b2`), with
  package count unchanged at 597. Re-confirmed on crates.io immediately
  before the bump: both versions still published, un-yanked, checksums
  matching. All named checks passed: `zfb-content` harness 18/18 +
  `error_messages`, 57/57 protected md-wasm tests (`api.rs` 32 +
  `parse_to_ast.rs` 25, both files in full — the bump topic ran only the
  two named pins; the review pass re-ran the pair, matching the 0.0.31
  record's count),
  12/12 `zfb` diagnostics unit tests (`--no-default-features`), the
  `wasm32-unknown-unknown` target check, `cargo deny check` clean, `pnpm
  test:md-wasm`, `node scripts/assert-md-wasm-size-docs.mjs`, `pnpm
  format:check`, and `cargo fmt --check`. The harness label pair
  (`CURRENT_ADAPTER_NAME` and the fixture's `adapter` field) was relabelled to
  `0.0.43` in lockstep; the corpus and the 18 baseline cases are untouched.
  The Mac build's measured wasm bytes differed from this repository's
  committed manifest for reasons unrelated to the pin (platform-specific
  `wasm-opt`/`wasm-bindgen` codegen, not a semantic regression — the #2988
  evaluation's Mac-measured deltas do not reproduce byte-for-byte on CI), so
  `crates/zfb-md-wasm/shipped-sizes.json` and its eight synced doc tables
  were provisionally refreshed from this Mac build (`measuredOnVersion` stays
  `2.15.0`); every artifact stayed under its ceiling, with `render-only`
  gzip-9 clearing its 1,100,000 B ceiling by only 4,106 B locally — the
  headroom the #2988 evaluation projected, and the artifact nearest its
  budget. **Those Mac bytes are not the final manifest.** As with the 0.0.31
  bump, the `wasm-md (default)` job asserts equality against its own ubuntu
  build, so before this PR merges the manifest must be aligned from the PR's
  own CI build summary and the doc tables re-synced via
  `assert-md-wasm-size-docs.mjs --fix`. `node scripts/check-yaml-candidate-drift.mjs
  --json` was re-run after refreshing `scripts/yaml-candidate-baseline.json`
  (write-then-copy from a fresh `--snapshot`) and exits 0 with `no-drift` on
  every candidate, including the adopted pair. **Next trigger:** unchanged
  from above — the next lockstep release beyond 0.0.43, or a yank of either
  adopted version.

  **Re-evaluated 2026-09-19 at `0.0.44` in
  [#3045](https://github.com/Takazudo/zudo-front-builder/issues/3045) —
  MIGRATE.** The weekly lane's `CANDIDATE_DRIFT` was reproduced locally at
  `2026-09-18T23:33:10.386Z` (rc 10, `errors: []`): on `noyalib`
  `version-published 0.0.44`, `version-published 0.0.45`, `tag-added v0.0.44`,
  `tag-added v0.0.45`, `release-added v0.0.44`, `release-added v0.0.45`, plus
  informational `branch-added feat/examples-autodiscovery`, `branch-added
  fix/kani-lint-conflict` and `branch-advanced main`
  (`a939aae03cb142f09ea5b9476e70218065e8ca79` →
  `bb63ec542e72a02a278308a93861af4fed63ea46`); on `noyalib-serde-yaml`
  `version-published 0.0.44`, `tag-added v0.0.44`, `release-added v0.0.44`,
  plus informational `branch-advanced main`
  (`02bc46287a04e2e66bed90c4836b6afb8af359d0` →
  `77d339e46532324482d68cf534a71709e0470fd6`), `branch-added
  release/v0.0.44` (`707fd753991513753cfc96da97c9d626aeb0f8ab`),
  `branch-added release/v0.0.45` (`8a6df29cadca2c4ae61981ca7114bfdc2d5f9702`),
  `branch-added dependabot/github_actions/github-actions-275b0fd278`,
  `branch-deleted ci/regression-gates`, and `branch-deleted` on the twelve
  spent release branches `feat/v0.0.32` through `feat/v0.0.43`. The
  non-adopted candidates carried informational deltas only (`saphyr`
  `branch-diverged renovate/ordered-float-5.x-lockfile`; `serde-saphyr`
  `version-published 1.3.0` and its tag/release), which trigger no
  evaluation.

  **Passed over: `noyalib 0.0.45`** (published 2026-09-18T18:05:28Z, sha256
  `10912f3af95a00e49a000323539768b9209a5948d76389fc2795c09f4fa6ed7d`,
  un-yanked). `noyalib-serde-yaml 0.0.45` does not exist on crates.io — only
  an upstream `release/v0.0.45` branch does — so 0.0.45 is not a lockstep
  pair and the newest-lockstep rule selects `0.0.44`. The authorized sha256
  pair for that release is
  `f307578cc6389d7c00bc284401ee1bff6d47c66c39debd8fc4db5b02892bc7a4`
  (`noyalib 0.0.44`, published 2026-09-17T00:34:55Z) and
  `66b663f6da0ea43cb511594d88bcd86159a6655452924f9b2f4026c951c10d65`
  (`noyalib-serde-yaml 0.0.44`, published 2026-09-17T01:04:26Z); the alias's
  own manifest pins exact `noyalib =0.0.44`. Re-checked immediately before
  recording this verdict at `2026-09-18T23:56:15Z`: both versions still
  published and un-yanked, both checksums unchanged, the alias pin still
  exact, and `noyalib-serde-yaml 0.0.45` still absent.

  **P1 — differential harness.** Under a temporary local pin to `=0.0.44`,
  `cargo test -p zfb-content` passed 1072/1072 across 34 binaries, including
  the immutable-baseline match **18/18**
  (`current_serde_yaml_matches_immutable_baseline`), the category coverage
  pin, the BOM/CRLF/emoji document contract, and the hard location pins. The
  corpus, the 18 baseline observations, and every protected assertion were
  left unedited, and the harness label pair (`CURRENT_ADAPTER_NAME` and the
  fixture's `adapter` field) was deliberately left reading `0.0.43` — the
  relabel belongs to the pin-bump topic, not to this evaluation.

  **Phase 2.** Every named check passed: `cargo check -p zfb-md-wasm --target
  wasm32-unknown-unknown` OK; 57/57 protected md-wasm tests (`api.rs` 32 +
  `parse_to_ast.rs` 25, both files in full); 12/12 `zfb` diagnostics unit
  tests under `--no-default-features`; `cargo deny check` clean with no new
  exception (`advisories ok, bans ok, licenses ok, sources ok`); 219/219
  `pnpm test:md-wasm` vitest tests across 12 files — its build step's
  `render-only` ceiling check is red on this host at **both** pins, which the
  finding below separates from the pin; `node
  scripts/assert-md-wasm-size-docs.mjs` OK; `pnpm format:check` green. The
  lock delta is the two adopted entries only — no transitive dependency
  entered or left (`noyalib`'s own dependency list is byte-identical) and the
  package set is unchanged at 597, name-for-name. No license-category change:
  both crates stay `MIT OR Apache-2.0` at `rust-version 1.86.0`, edition
  2024.

  **Measured wasm bytes (local, `Darwin arm64`).** Unlike the 0.0.43 step,
  this bump is effectively a size no-op — gzip-9 moves by at most 163 B and
  mostly downward:

  | artifact | `finalWasm` 0.0.43 → 0.0.44 | Δ | `gzip9` 0.0.43 → 0.0.44 | Δ |
  | --- | --- | --- | --- | --- |
  | root | 3,473,319 → 3,473,330 | +11 | 1,553,889 → 1,553,887 | −2 |
  | highlight-only | 1,543,239 → 1,543,455 | +216 | 825,575 → 825,579 | +4 |
  | render-only | 2,241,373 → 2,241,085 | −288 | 1,113,363 → 1,113,318 | −45 |
  | parse-only | 730,672 → 730,338 | −334 | 297,535 → 297,372 | −163 |

  Both glue columns (`glue`, `glueGzip9`) are byte-identical at both pins. The
  pin bump must still refresh the size manifest from its own CI build summary
  rather than from these numbers, for the reason the 0.0.31 and 0.0.43 records
  already give.

  **Finding, not a blocker: the local md-wasm build gate is red at HEAD on
  this Mac, independent of the pin.** `pnpm test:md-wasm` fails its
  `render-only gzip-9 ... exceeds ceiling 1100000` check at `0.0.44`
  (1,113,318 B) — and reproduces at the **committed `0.0.43` pin**
  (1,113,363 B, 45 B *larger*), so it is not a `0.0.44` regression. It is the
  documented Mac-versus-CI codegen gap, wider here than the up-to-5,982 B the
  0.0.31 record measured: local render-only gzip-9 runs 21,748 B above the
  committed CI manifest's 1,091,615 B, and local parse-only 13,270 B above its
  284,265 B. No ceiling was raised and no assertion was edited to get past it;
  the 219 vitest tests above were run against the artifacts that build writes
  before its summary aborts. Applying this evaluation's −45 B render-only
  delta to CI's committed number leaves ~1,091,570 B, still inside the
  1,100,000 B ceiling — `render-only` remains the artifact nearest its budget
  and the CI build summary is the only authority on whether it clears.

  **Toolchain:** rustc 1.93.1 (01f6ddf75 2026-02-11), cargo 1.93.1, wasm-bindgen
  0.2.121, Node v24.13.0, pnpm 11.3.0, `Darwin arm64`.

  **Verdict: MIGRATE to the lockstep `noyalib 0.0.44` /
  `noyalib-serde-yaml 0.0.44` pair.** The pin bump is the next topic's job;
  `Cargo.toml` and `Cargo.lock` are unchanged by this evaluation. **Next
  trigger:** the next `version-published`, `tag-added`, or `release-added`
  delta on a lockstep `noyalib` + `noyalib-serde-yaml` release beyond 0.0.44 —
  `noyalib 0.0.45` already qualifies the moment `noyalib-serde-yaml 0.0.45`
  publishes — or a yank of either adopted version. Run the evaluation protocol
  against it and never refresh the detector baseline over it.

  **The 0.0.44 bump landed 2026-09-19 in the pin-bump topic
  ([#3046](https://github.com/Takazudo/zudo-front-builder/issues/3046)), refs
  [#3045](https://github.com/Takazudo/zudo-front-builder/issues/3045).** Root
  `Cargo.toml` now pins `serde_yaml = { package = "noyalib-serde-yaml",
  version = "=0.0.44" }`; zero production `.rs` lines changed under the
  abandon rule, so every `serde_yaml::` call site in `zfb-content`, `zfb`, and
  `zfb-md-wasm` still compiles unchanged. `Cargo.lock` moved exactly the two
  adopted entries — `noyalib` and `noyalib-serde-yaml` — from `0.0.43` to
  `0.0.44`, checksums equal to the authorized sha256 pair
  (`f307578cc6389d7c00bc284401ee1bff6d47c66c39debd8fc4db5b02892bc7a4` and
  `66b663f6da0ea43cb511594d88bcd86159a6655452924f9b2f4026c951c10d65`), with
  package count unchanged at 597. All named checks passed: `zfb-content`
  harness 18/18 + the corpus-coverage/BOM-CRLF-emoji/hard-location-pin tests
  (1072/1072 across 34 binaries), 57/57 protected md-wasm tests (`api.rs` 32 +
  `parse_to_ast.rs` 25), 12/12 `zfb` diagnostics unit tests
  (`--no-default-features`), the `wasm32-unknown-unknown` target check,
  `cargo deny check` clean, 219/219 `pnpm test:md-wasm` vitest tests (run
  directly via `pnpm --filter @takazudo/zfb-md-wasm test`, since the chained
  `build && test` script's build step aborts on the pre-existing local ceiling
  breach below — the build writes its artifacts before that abort), `node
  scripts/assert-md-wasm-size-docs.mjs` (green apart from the one
  local-ceiling-breach exception below), `pnpm format:check`, and
  `cargo fmt --check`. The harness label pair (`CURRENT_ADAPTER_NAME` and the
  fixture's `adapter` field) was relabelled to `0.0.44` in lockstep; the
  corpus and the 18 baseline cases are untouched.

  The Mac build's measured wasm bytes differed from this repository's
  committed manifest for reasons unrelated to the pin (platform-specific
  `wasm-opt`/`wasm-bindgen` codegen, not a semantic regression — the #3045
  evaluation's Mac-measured deltas between 0.0.43 and 0.0.44 do not reproduce
  byte-for-byte on CI). Measured locally on `Darwin arm64`: root 3,473,330 /
  1,553,887 B, highlight 1,543,455 / 825,579 B, render 2,241,085 /
  1,113,318 B, parse 730,338 / 297,372 B (final wasm / gzip-9) — and
  **`render-only` measures 13,318 B *over* its 1,100,000 B gzip-9 ceiling on
  this Mac**, the same pre-existing Mac-vs-CI codegen gap #3045 already
  documented at the committed 0.0.43 pin (which measures 1,113,363 B locally,
  45 B larger than 0.0.44's 1,113,318 B), now simply wider than the up-to-
  5,982 B gap the 0.0.31 record measured and the 4,106-B-under-ceiling gap the
  0.0.43 record measured.

  **Those Mac bytes are recorded here only, and deliberately NOT written into
  `crates/zfb-md-wasm/shipped-sizes.json`.** An earlier revision of this topic
  did refresh the manifest and its eight synced doc tables from this Mac build
  and hand-edited the six "of headroom" sentences to narrate the local ceiling
  breach; that was reverted, because the manifest is the CI oracle, not a
  local-measurement scratchpad. Two concrete consequences made it
  unshippable: `node scripts/assert-md-wasm-size-docs.mjs` — a step in the
  **required** `health` job, not an optional lane — exited 1 on the
  hand-edited prose, and the `wasm-md (default)` job asserts manifest equality
  against its own ubuntu build with zero tolerance on the two `finalWasm`
  columns, so Mac bytes are a guaranteed `manifest-mismatch`. No ceiling was
  raised, no assertion was edited to silence anything, and the corpus was
  untouched.

  **Reconciled from CI (PR #3048, run 35410352499).** The `wasm-md (default)`
  job's own ubuntu build settled the numbers, and the manifest plus its eight
  synced doc tables were realigned from that summary (doc tables re-synced via
  `assert-md-wasm-size-docs.mjs --fix`). The 0.0.44 pin does move the shipped
  bytes, slightly and in both directions:

  | Artifact | final wasm | Δ | gzip-9 | Δ |
  | --- | --- | ---: | --- | ---: |
  | root (full) | 3,400,172 → 3,399,954 | −218 | 1,516,626 → 1,516,383 | −243 |
  | highlight-only | 1,538,664 → 1,539,334 | +670 | 817,835 → 817,951 | +116 |
  | render-only | 2,196,673 → 2,196,095 | −578 | 1,091,615 → 1,091,678 | +63 |
  | parse-only | 701,306 → 700,364 | −942 | 284,265 → 283,991 | −274 |

  Every artifact stays under its ceiling on CI, `render-only` most narrowly at
  1,091,678 B against 1,100,000 B. `measuredOnVersion` stays `2.15.0`,
  matching the precedent reconciliation in #2989 / PR #2992 — it labels the
  measurement epoch the doc tables cite, and is deliberately not asserted
  against the package version. The local Mac figures above therefore stand as
  recorded evidence of the host gap (~21.7 KB on `render-only`), not as
  shipped numbers; that widened gap is tracked separately in #3049.
  **Next trigger:** unchanged from above — the next lockstep release beyond
  0.0.44, or a yank of either adopted version.

  Earlier rounds decided the other way, and that history stands. The released
  candidates evaluated in #2787 and #2788 each diverged from the committed
  `serde_yaml` baseline in 11 of 18 cases. A pinned pre-release re-check of
  noyalib's post-cutoff `feat/v0.0.29` branch in #2808 still found 11
  mismatches through its compatibility shim and 9 through the closest
  configured direct path, and the released 0.0.29 pair in #2836 reached 17/18
  with `custom-explicit-tag` still anchored at `1:16:15`. Concrete JSON,
  resource-limit, error-rendering, and diagnostic-location blockers outweighed
  the candidates' maintenance advantage in every one of those rounds; upstream
  closed the last of them in 0.0.30. Nothing in CI ever forced the migration:
  `deny.toml` has no `serde_yaml` or `unsafe-libyaml` exception or advisory
  entry, and advisories are the security workflow's only deny-on-finding
  section. The surface being moved is correspondingly tiny: one frontmatter
  `from_str::<serde_json::Value>` call, one `#[from]` error conversion, and
  `Error::location()` consumers in `zfb` and `zfb-md-wasm`; `serde_yaml::Value`
  is not used.

  The standing abandon rule makes a migration terminally KEEP if it needs more
  than 40 rustfmt-formatted non-test production lines, a second corrective
  candidate round, or unsafe/FFI, OS-specific branches, polling, signal
  supervision, or a generated Unicode table. Neither evaluated candidate hit
  those mechanical thresholds: the noyalib compatibility port was 2
  production lines, its configured direct path was 5, and the serde-saphyr
  port was 10, each in one candidate round and without a prohibited
  mechanism. Their semantic incompatibilities decided those results before
  Phase 2, so no wasm size, transitive-package, or license delta was measured
  in any of them; #2851 is the first round to reach Phase 2 and therefore the
  first to warrant a migration topic and changelog entries.
  [#2755](https://github.com/Takazudo/zudo-front-builder/issues/2755) remains
  open and is not closed by the migration. (Superseded 2026-09-03: #2755 was
  closed with a terminal comment once the migration had merged; the watch's
  protocol now lives in the "YAML candidate watch" lane paragraph below.)

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
  * **`noyalib` — evaluated; migrated:** the lockstep `noyalib 0.0.30` /
    **`noyalib-serde-yaml`** `0.0.30` pair was published on 2026-09-02 and is
    the release the standing trigger named. The
    [noyalib tag](https://github.com/sebastienrousseau/noyalib/releases/tag/v0.0.30)
    resolves to `e33ba3c90a02721388e974b458f07eeb0b40198a`,
    [release PR 371](https://github.com/sebastienrousseau/noyalib/pull/371) is
    merged, and the
    [alias tag](https://github.com/sebastienrousseau/noyalib-serde-yaml/releases/tag/v0.0.30)
    resolves to `f12787c737bef8b579265fde52cd09696c60cfd6` with the alias
    pinning exact `noyalib =0.0.30`. Upstream's shipped contract now asserts
    `custom-explicit-tag` at `1:8:7` with Display column 8, which is the pin
    #2755 required. Both the zero-line package alias and two-line direct shim
    matched **18/18** immutable corpus cases and every protected assertion, and
    Phase 2 was green, so the verdict is MIGRATE
    ([#2851](https://github.com/Takazudo/zudo-front-builder/issues/2851)). The
    preceding `0.0.29` round is the contrast: both paths reached only **17/18**
    there, with `custom-explicit-tag` anchored at `1:16:15` and Display column
    16, which is why its verdict was RETAIN
    ([#2836](https://github.com/Takazudo/zudo-front-builder/issues/2836)).
    The migration landed 2026-09-03 in the migration topic
    ([#2852](https://github.com/Takazudo/zudo-front-builder/issues/2852)); see
    the `serde_yaml 0.9.34+deprecated` summary bullet above for the landed
    diff, checksums, and lock delta.

    **Bumped to `0.0.31` the same day** in the pin-bump topic
    ([#2875](https://github.com/Takazudo/zudo-front-builder/issues/2875)),
    following the 18/18 MIGRATE verdict of the released differential
    evaluation ([#2873](https://github.com/Takazudo/zudo-front-builder/issues/2873));
    see the `serde_yaml 0.9.34+deprecated` summary bullet above for the
    landed diff, checksums, and lock delta.

    **Bumped to `0.0.43` on 2026-09-14** in the pin-bump topic
    ([#2989](https://github.com/Takazudo/zudo-front-builder/issues/2989)),
    following the 18/18 MIGRATE verdict of the released differential
    evaluation ([#2988](https://github.com/Takazudo/zudo-front-builder/issues/2988));
    see the `serde_yaml 0.9.34+deprecated` summary bullet above for the
    landed diff, checksums, and lock delta.

    Recorded branch-churn triage (live check
    `2026-09-02T17:02:57.258Z`): the detector returned rc 10 with no errors and
    only `noyalib` branch deltas. `feat/v0.0.30` advanced from
    `17790c55f19301af0c6e94c8bfceb1dc914086c1` to
    `e5f4e7b1498c7a7b990971c5f0a4c48e6343c0bb`, whose 2026-09-02T16:14:05Z
    subject is _“style: rustfmt the span-widening closure and contract pins”_;
    `feat/v0.0.31` advanced from
    `75e46581e5dbf3e234813aaaa00f71184f784bae` to
    `a0bb22ec1ce1bf80ddc65e5e83ba8c4eeaa634eb`, the 2026-09-02T13:41:42Z
    _“Merge branch 'feat/v0.0.31' into feat/v0.0.33”_ commit; and
    `feat/v0.0.33` was deleted at that same full SHA. A review-time recheck at
    `2026-09-02T17:11:36.674Z` again returned rc 10 with no errors and one
    additional allowed delta: `feat/v0.0.31` advanced from
    `a0bb22ec1ce1bf80ddc65e5e83ba8c4eeaa634eb` to
    `0fa6c414a4a73c99a6df533d206f73158d80cab8`, the 2026-09-02T17:08:46Z
    _“Merge branch 'feat/v0.0.30' into feat/v0.0.31”_ commit. Neither
    observation contained a `version-published`, `tag-added`, `release-added`,
    or archive delta for any candidate, and the re-checked crates.io maxima
    remained `0.0.29` for both `noyalib` and `noyalib-serde-yaml`. At the
    observed `feat/v0.0.30` head, the upstream contract still asserts Display
    column 8 and `1:8:7` _“since v0.0.30”_; tag `v0.0.29` retains the _KNOWN
    PARTIAL_ Display column 16 / `1:16:15` behavior. This is release-prep branch
    churn, so the standing
    [#2755](https://github.com/Takazudo/zudo-front-builder/issues/2755) trigger
    has not fired: it requires a lockstep release carrying that pin. As part of
    this recorded triage, the baseline was refreshed at
    `2026-09-02T17:12:41.295Z`, and the tracked release PR was re-pointed from
    merged PR 365 to observed-open
    [PR 371](https://github.com/sebastienrousseau/noyalib/pull/371). The next
    `version-published`, `tag-added`, or `release-added` delta for a lockstep
    `noyalib` + `noyalib-serde-yaml` 0.0.30 release **is** the trigger: run the
    evaluation protocol and never refresh over it. Further branch-only churn
    while upstream prepares 0.0.30/0.0.31 requires one recorded-triage refresh
    per observation, never a refresh loop. (Superseded 2026-09-03: the
    detector now classifies branch-only churn as `informational-drift`, exit
    0, which needs neither a recorded-triage refresh nor a tracking issue;
    see the lane paragraph below.)

    That predicted trigger then fired, and the deltas below were **evaluated by
    the #2851 section further down this file, not refreshed over**. The live
    check at `2026-09-02T19:52:52.555Z` returned rc 10 with no errors, and an
    identical re-check at `2026-09-02T20:24:42.866Z` returned the same set.
    `noyalib` reported `version-published 0.0.30`, `tag-added v0.0.30`,
    `release-added v0.0.30`, `release-pr-state-changed` for
    [PR 371](https://github.com/sebastienrousseau/noyalib/pull/371) from OPEN to
    MERGED, `branch-deleted feat/v0.0.30` at
    `e5f4e7b1498c7a7b990971c5f0a4c48e6343c0bb`, `branch-advanced main` from
    `9dca232d7014b853a1c25e0768ddd12afa2873a2` to
    `e33ba3c90a02721388e974b458f07eeb0b40198a`, and `branch-advanced
    feat/v0.0.31` from `0fa6c414a4a73c99a6df533d206f73158d80cab8` to
    `50b13d299a87668b8209a4c6b5012e3eb4d2a32f`, whose 2026-09-02T19:42:53Z
    subject is _"Merge remote-tracking branch 'origin/main' into
    feat/v0.0.31"_. `noyalib-serde-yaml` reported `version-published 0.0.30`,
    `tag-added v0.0.30`, `release-added v0.0.30`, `branch-advanced main` from
    `11cffda345303d9e19cadd3e297a4506818a4d01` to
    `f12787c737bef8b579265fde52cd09696c60cfd6`, and `branch-added
    release/v0.0.30` at that same SHA; its `feat/v0.0.31`
    ([PR 3](https://github.com/sebastienrousseau/noyalib-serde-yaml/pull/3),
    docs/structure) remains open at
    `d6e29a7b048f630f3bda691de489515fe9a158e8`. The other four tracked
    candidates reported no drift. **#2851 performs no baseline refresh and no
    detector edit**: the single late baseline refresh and the `pendingReleasePr`
    re-point away from now-merged PR 371 are performed together, atomically, by
    the confirm topic
    ([#2853](https://github.com/Takazudo/zudo-front-builder/issues/2853)) of
    epic [#2850](https://github.com/Takazudo/zudo-front-builder/issues/2850),
    as late as possible.

    The confirm topic ([#2853](https://github.com/Takazudo/zudo-front-builder/issues/2853))
    then performed that single late refresh. A live check at
    `2026-09-02T21:07:22.080Z` (config still pointing `pendingReleasePr` at
    merged PR 371) returned rc 10 with no errors and, versus the
    `2026-09-02T17:12:41.295Z` baseline, exactly the following deltas: for
    `noyalib` — `version-published 0.0.30`, `tag-added v0.0.30`,
    `release-added v0.0.30`, `branch-deleted feat/v0.0.30` at
    `e5f4e7b1498c7a7b990971c5f0a4c48e6343c0bb`, `branch-advanced
    feat/v0.0.31` from `0fa6c414a4a73c99a6df533d206f73158d80cab8` to
    `a1fb50918d8b7484b928118c9adffe852aacc5f1`, `branch-advanced main` from
    `9dca232d7014b853a1c25e0768ddd12afa2873a2` to
    `e33ba3c90a02721388e974b458f07eeb0b40198a`, and
    `release-pr-state-changed` for PR 371 from OPEN to MERGED; for
    `noyalib-serde-yaml` — `version-published 0.0.30`, `tag-added v0.0.30`,
    `release-added v0.0.30`, `branch-advanced main` from
    `11cffda345303d9e19cadd3e297a4506818a4d01` to
    `f12787c737bef8b579265fde52cd09696c60cfd6`, and `branch-added
    release/v0.0.30` at that same SHA. The other five tracked candidates
    reported no drift. Every delta was classified as either the
    already-evaluated 0.0.30 release set #2851 recorded above or ordinary
    `main`/`feat/*` branch churn — the `feat/v0.0.31` head had simply
    advanced again past the #2851 observation; none was an un-evaluated
    trigger-kind delta (no version/tag/release beyond 0.0.30 for either
    crate, no yank, and no drift on `serde_yaml_ng`, `serde_yml`, `saphyr`,
    `serde_norway`, or `serde-saphyr`), so the step-2 gate did not fire.
    `CANDIDATE_CONFIG.noyalib.pendingReleasePr` was then set to `null` in
    `scripts/check-yaml-candidate-drift.mjs`. `sebastienrousseau/noyalib`'s
    open pull requests at that point were only
    [PR 374](https://github.com/sebastienrousseau/noyalib/pull/374)
    (`feat/v0.0.31`, _"chore(structure)!: v0.0.31: docs layout, User Manual,
    install UX groundwork, contributor DX"_ — a docs/structure PR, the same
    pattern already recorded above for the alias repo's open PR 3) and
    [PR 373](https://github.com/sebastienrousseau/noyalib/pull/373) (an
    unrelated CST-parsing feature PR), neither a release PR, so no PR number
    was substituted. A second, fresh observation immediately after the
    config edit, at `2026-09-02T21:08:22.477Z`, reported the identical delta
    set except that `noyalib`'s PR-tracking delta became `release-pr-changed`
    from `{ number: 371, state: "OPEN" }` to `null` — the direct, expected
    consequence of the config edit — so re-classifying this second report
    changed nothing and the gate still did not fire. That second
    observation's snapshot was installed as the new
    `scripts/yaml-candidate-baseline.json` (`checkedAt`
    `2026-09-02T21:08:22.477Z`), config and baseline now agreeing on a
    `null` `pendingReleasePr` for `noyalib`. A post-install re-run at
    `2026-09-02T21:08:56.542Z` returned exit 0, `no-drift`, `errors: []`.
    **This is the single refresh of this triage; further upstream churn
    after it gets no second refresh** — the next `version-published`,
    `tag-added`, or `release-added` delta for either crate beyond 0.0.30, a
    yank, or any newly observed-open `noyalib` release PR is fresh
    trigger-kind evidence for a new evaluation topic, not grounds to refresh
    this baseline again.

    The paragraph above names exactly this as fresh trigger-kind evidence for
    a new evaluation topic, and it arrived one day later. The deltas below are
    **evaluated by the #2873 section further down this file, not refreshed
    over**. The planning observation at `2026-09-03T11:00:03.903Z` returned
    rc 10 `CANDIDATE_DRIFT` with `errors: []`, and this topic's own re-check
    at `2026-09-03T11:49:34.325Z` — repeated with an authenticated token at
    `2026-09-03T11:53:10.516Z` — returned the same delta kinds on the same
    branches, differing only in one SHA noted below. For
    `noyalib`: `version-published 0.0.31`, `tag-added v0.0.31`,
    `release-added v0.0.31`, `branch-deleted feat/v0.0.31` at
    `a1fb50918d8b7484b928118c9adffe852aacc5f1`, `branch-added feat/v0.0.32`
    at `3dcd79fdf819ce0bef281be4174c28d55eeba31f` at planning and at
    `ac0beb63abbdfbe6fad7b3739241cac4f7d0ba76` in this topic's runs — the one
    difference between the two observations, an unreleased feature branch
    advancing — and `branch-advanced main`
    from `e33ba3c90a02721388e974b458f07eeb0b40198a` to
    `f57afdd8d2ef2645578a80b089b3485bcc72b633`. For `noyalib-serde-yaml`:
    `version-published 0.0.31`, `tag-added v0.0.31`, `release-added v0.0.31`,
    `branch-deleted feat/v0.0.30` at
    `58a23f239c061d898230748fc0745da67ca3e978`, `branch-deleted feat/v0.0.31`
    at `d6e29a7b048f630f3bda691de489515fe9a158e8`, `branch-deleted
    release/v0.0.30` at `f12787c737bef8b579265fde52cd09696c60cfd6`, and
    `branch-advanced main` from `f12787c737bef8b579265fde52cd09696c60cfd6` to
    `27fbdd9e54c982df09d4ffda6de0267a36c9ab4d`. The five candidate-role
    crates reported `no-drift`, and neither repository had an open pull
    request, so `pendingReleasePr` correctly remains `null`. The
    `version-published` / `tag-added` / `release-added` triple on both
    adopted crates is trigger-kind evidence and was answered by running the
    evaluation protocol; **#2873 performs no baseline refresh and no detector
    edit**. The single late baseline refresh for this round is performed
    once, atomically, by the confirm topic
    ([#2876](https://github.com/Takazudo/zudo-front-builder/issues/2876)) of
    epic [#2872](https://github.com/Takazudo/zudo-front-builder/issues/2872),
    as late as possible.

    The confirm topic ([#2876](https://github.com/Takazudo/zudo-front-builder/issues/2876))
    then performed that single late refresh. A live check at
    `2026-09-03T13:44:01.642Z` (config still pointing `pendingReleasePr` at
    `null`) returned rc 10 with no errors and, versus the
    `2026-09-02T21:08:22.477Z` baseline, exactly the following deltas: for
    `noyalib` — `version-published 0.0.31`, `tag-added v0.0.31`,
    `release-added v0.0.31`, `branch-deleted feat/v0.0.31` at
    `a1fb50918d8b7484b928118c9adffe852aacc5f1`, `branch-added feat/v0.0.32`
    at `ac0beb63abbdfbe6fad7b3739241cac4f7d0ba76`, and `branch-advanced main`
    from `e33ba3c90a02721388e974b458f07eeb0b40198a` to
    `f57afdd8d2ef2645578a80b089b3485bcc72b633`; for `noyalib-serde-yaml` —
    `version-published 0.0.31`, `tag-added v0.0.31`, `release-added v0.0.31`,
    `branch-deleted feat/v0.0.30` at
    `58a23f239c061d898230748fc0745da67ca3e978`, `branch-deleted feat/v0.0.31`
    at `d6e29a7b048f630f3bda691de489515fe9a158e8`, `branch-deleted
    release/v0.0.30` at `f12787c737bef8b579265fde52cd09696c60cfd6`, and
    `branch-advanced main` from `f12787c737bef8b579265fde52cd09696c60cfd6` to
    `27fbdd9e54c982df09d4ffda6de0267a36c9ab4d`. The five candidate-role
    crates reported `no-drift`. Every delta was classified as either the
    already-evaluated `0.0.31` release set #2873 recorded above (the
    `version-published` / `tag-added` / `release-added` triple on both
    adopted crates) or ordinary branch churn (`branch-added`,
    `branch-deleted`, `branch-advanced`, including the new `feat/v0.0.32`
    branch); none was an un-evaluated trigger-kind delta (no version/tag/
    release beyond `0.0.31` for either crate, no yank, and no drift on
    `serde_yaml_ng`, `serde_yml`, `saphyr`, `serde_norway`, or
    `serde-saphyr`), so the step-2 gate did not fire.
    `gh api "repos/sebastienrousseau/noyalib/pulls?state=open"` returned no
    open pull requests at check time, so
    `CANDIDATE_CONFIG.noyalib.pendingReleasePr` correctly remains `null` and
    needed no edit. The
    observed snapshot (`checkedAt` `2026-09-03T13:44:01.642Z`) was installed
    as the new `scripts/yaml-candidate-baseline.json` via the write-then-copy
    recipe. A post-install re-run at `2026-09-03T13:44:42.933Z` returned exit
    0, `no-drift`, `errors: []`. **This is the single refresh of this
    triage; further upstream churn after it gets no second refresh** — the
    next `version-published`, `tag-added`, or `release-added` delta for
    either crate beyond `0.0.31`, a yank, or any newly observed-open
    `noyalib` release PR is fresh trigger-kind evidence for a new evaluation
    topic, not grounds to refresh this baseline again.

    **Superseded 2026-09-13 by
    [#2988](https://github.com/Takazudo/zudo-front-builder/issues/2988).** The
    eleven releases 0.0.32-0.0.42 went by un-evaluated and `0.0.43` was
    evaluated in their place with verdict MIGRATE, so the standing next
    trigger now reads: the next `version-published`, `tag-added`, or
    `release-added` delta for either crate **beyond `0.0.43`**, a yank of
    either adopted version, or any newly observed-open `noyalib` release PR —
    `pendingReleasePr` stayed `null` through #2988 because all four open
    `noyalib` pull requests at check time were ordinary CST `fix/`/`feat/`
    branches. That evidence opens a new evaluation topic; it is still not
    grounds to refresh this baseline again.

    **Superseded 2026-09-19 by
    [#3045](https://github.com/Takazudo/zudo-front-builder/issues/3045).** The
    weekly lane paged on the adopted pair's `0.0.44` release (and on
    `noyalib 0.0.45`, whose alias half is unpublished); `0.0.44` was evaluated
    with verdict MIGRATE, so the standing next trigger now reads: the next
    `version-published`, `tag-added`, or `release-added` delta for either
    crate **beyond `0.0.44`** — including `noyalib 0.0.45` once
    `noyalib-serde-yaml 0.0.45` publishes — a yank of either adopted version,
    or any newly observed-open `noyalib` release PR. `pendingReleasePr` stayed
    `null` through #3045. That evidence opens a new evaluation topic; it is
    still not grounds to refresh this baseline again.

    **Bumped to `0.0.44` on 2026-09-17** in the pin-bump topic
    ([#3046](https://github.com/Takazudo/zudo-front-builder/issues/3046)),
    following the 18/18 MIGRATE verdict of the released differential
    evaluation ([#3045](https://github.com/Takazudo/zudo-front-builder/issues/3045));
    see the `serde_yaml 0.9.34+deprecated` summary bullet above for the
    landed diff, checksums, and lock delta.

    The confirm topic
    ([#3047](https://github.com/Takazudo/zudo-front-builder/issues/3047))
    then performed the single refresh for this round. A live check at
    `2026-09-19T00:35:01.285Z` (config still pointing `pendingReleasePr` at
    `null`) returned rc 10 with no errors and, versus the
    `2026-09-13T21:40:41.816Z` baseline, exactly the following triage-severity
    deltas: for `noyalib` — `version-published 0.0.44`, `version-published
    0.0.45`, `tag-added v0.0.44`, `tag-added v0.0.45`, `release-added
    v0.0.44`, `release-added v0.0.45`; for `noyalib-serde-yaml` —
    `version-published 0.0.44`, `tag-added v0.0.44`, `release-added v0.0.44`.
    Both crates also reported ordinary branch churn (`branch-added`,
    `branch-deleted`, `branch-advanced`), including `noyalib-serde-yaml`
    gaining a `release/v0.0.45` branch with no matching `version-published` —
    branch movement is informational, not a trigger, so this is not the
    lockstep pair appearing. Every triage-severity delta was already
    evaluated: the `0.0.44` triple on both crates is the MIGRATE verdict
    #3045 recorded and #3046 landed, and the bare `noyalib 0.0.45` triple is
    the already-documented, deliberately-deferred half-pair from the
    paragraph above (its alias, `noyalib-serde-yaml 0.0.45`, still has no
    `version-published` delta, so the pair is still incomplete and this
    round folds in no new unevaluated release). The five candidate-role
    crates reported only informational drift (new versions/tags/branches),
    none of it on the adopted pair. `gh api
    "repos/sebastienrousseau/noyalib/pulls?state=open"` returned one open
    pull request, [PR 445](https://github.com/sebastienrousseau/noyalib/pull/445)
    (`feat/examples-autodiscovery`, an examples-discovery refactor, not a
    release PR), so `CANDIDATE_CONFIG.noyalib.pendingReleasePr` correctly
    remains `null` and needed no edit. The observed snapshot (`checkedAt`
    `2026-09-19T00:35:01.285Z`) was installed as the new
    `scripts/yaml-candidate-baseline.json` via the write-then-copy recipe. A
    post-install re-run at `2026-09-19T00:35:25.788Z` returned exit 0,
    `no-drift`, `errors: []` on every candidate including both adopted
    crates. **This is the single refresh of this triage; further upstream
    churn after it gets no second refresh** — the next `version-published`,
    `tag-added`, or `release-added` delta for either crate **beyond
    `0.0.44`** (for `noyalib` this means anything past the now-baselined
    `0.0.45`), a yank of either adopted version, or any newly observed-open
    `noyalib` release PR is fresh trigger-kind evidence for a new evaluation
    topic — in particular, `noyalib-serde-yaml` publishing `0.0.45` would
    complete the lockstep pair and open that topic — not grounds to refresh
    this baseline again.
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
  and missed `noyalib`'s `feat/v0.0.29` release branch."* Every delta carries
  a severity. The nine triage-severity kinds — `version-published`,
  `version-yanked`, `version-unyanked`, `tag-added`, `release-added`,
  `release-pr-state-changed`, `release-pr-changed`, `repository-archived`,
  `repository-unarchived` — make the run `CANDIDATE_DRIFT` (exit 10) and open
  or append the deduped tracking issue, but only on the adopted pair
  (`noyalib`, `noyalib-serde-yaml`; the current pin lives in the root
  `Cargo.toml`, its history in the ledger above): severity is role-aware.
  The same nine kinds on any of the five candidates, plus the four branch
  kinds — `branch-added`, `branch-deleted`, `branch-advanced`,
  `branch-diverged` — and `version-record-touched` on every crate regardless
  of role, are still observed and listed but make the run
  `informational-drift` (exit 0), which closes or keeps closed the tracker
  exactly like `no-drift`; divergence is measured
  against the baseline head, which moves only at recorded triages, so a
  rewrite that preserves the baseline head as an ancestor is reported as
  `branch-advanced`, and a branch head that changed without ancestry evidence
  is still an operational failure — resolving it is itself a recorded triage
  that may refresh the baseline. A `CANDIDATE_DRIFT` on the adopted pair
  means: run `crates/zfb-content/tests/yaml_differential_harness.rs` against
  the new lockstep pair and record the verdict as a new evaluation topic (pin
  bump if 18/18 plus the Phase 2 checks, otherwise record the blocker). A
  delta on any of the five candidates is fallback-ledger information only —
  no evaluation is triggered unless the adopted pair itself degrades. The
  watcher never decides that evaluation's verdict; the baseline is refreshed
  only as part of a recorded triage, never merely to turn the lane green, and
  branch-only churn no longer requires one. The only refresh recipe is
  write-then-copy —
  `S=$(mktemp -d) && GITHUB_TOKEN=$(gh auth token) node scripts/check-yaml-candidate-drift.mjs --snapshot > "$S/snap.json" && cp "$S/snap.json" scripts/yaml-candidate-baseline.json && pnpm exec prettier --write scripts/yaml-candidate-baseline.json`
  — because the detector reads the baseline it is about to replace for
  branch-ancestry evidence, so `--snapshot` output must never be redirected
  onto the baseline path.

  **The between-runs tripwire (schemaVersion 3).** A weekly run compares a
  live observation against the committed baseline, so an event that happens
  and reverts between two runs leaves nothing to see. crates.io's per-version
  `updated_at` survives such a reversion, so the baseline persists it as
  `versionUpdatedAt` and the detector emits `version-record-touched` for any
  version whose timestamp moved without a yank-state change the same
  comparison already reports. A newly published version never produces one —
  it carries no earlier timestamp. The delta is informational on both roles
  and never pages: a transient leaves no state the exact pin and its
  `Cargo.lock` checksum depend on, and `updated_at` proves only that the
  record changed, never that a yank happened. Like `branch-advanced` it
  repeats every week until a recorded triage writes the observed timestamp
  into the baseline.
  The values were back-filled into the existing baseline at schemaVersion 3
  rather than taken from a refresh, and that back-fill is fully determined at
  the baseline's `checkedAt` (`2026-09-02T21:08:22.477Z`): each of the 99
  never-yanked versions had `updated_at == created_at`, and the only two
  yanked versions — `serde-saphyr` `0.0.9` (`2025-11-24T13:24:30.374170Z`)
  and `0.0.8-alpha-pre` (`2025-11-19T20:36:13.683080Z`) — carried exactly
  those timestamps, so no version had been touched after the baseline was
  taken. Source:
  [#2864](https://github.com/Takazudo/zudo-front-builder/issues/2864).

  **Acknowledging one record touch** a triage has looked at and wants to stop
  seeing is its own narrow recipe — never the full refresh above, which would
  silently absorb every other pending delta at the same time. It is
  write-then-copy for the same reason, changes exactly one timestamp, and is
  done only as part of a recorded triage note, under the same anti-gaming
  rule:

  `S=$(mktemp -d) && jq --arg c CANDIDATE --arg v VERSION --arg t OBSERVED_TS '.candidates[$c].versionUpdatedAt[$v] = $t' scripts/yaml-candidate-baseline.json > "$S/ack.json" && cp "$S/ack.json" scripts/yaml-candidate-baseline.json && pnpm exec prettier --write scripts/yaml-candidate-baseline.json`

  **Accepted blind spots (weekly cadence).** The tripwire closes the
  crates.io half only. A tag or a GitHub Release added and then deleted
  between two runs, and an archive followed by an unarchive, remain
  unobservable: neither surface exposes a history API the run could
  reconstruct them from. A release-PR flip is likewise unobservable while
  `pendingReleasePr` is `null`, because the watcher then queries no PR at all;
  when a PR number is pinned, the flip can be reconstructed at triage time
  from `/repos/{owner}/{repo}/issues/{n}/events`, whose `closed`, `reopened`
  and `merged` events carry timestamps. Option (3) in
  [#2864](https://github.com/Takazudo/zudo-front-builder/issues/2864) — a
  daily cadence — was rejected: seven times the runs to shrink the window,
  not to close it.

  Reconcile the vocabulary once: triage-severity is what the recorded
  triages above call trigger-kind evidence, with one exception: the "any
  newly observed-open `noyalib` release PR" clause is a human triage-time
  observation (re-point `pendingReleasePr` in
  `scripts/check-yaml-candidate-drift.mjs` when a triage happens), not
  something the detector observes; its former mechanical proxy — a new
  `feat/vX.Y.Z` branch — is now informational, and the adopted pair's release
  itself (`version-published` / `tag-added` / `release-added`, which
  noyalib's 1-2 day cadence delivers within days of such a branch) is the
  paging signal. While `pendingReleasePr` is `null` the watcher does not
  discover newly opened release PRs at all, so this clause cannot proactively
  trigger a triage before a publish/tag/release signal.

  **Recorded classification triage (2026-09-03):** workflow runs
  [33644242370](https://github.com/Takazudo/zudo-front-builder/actions/runs/33644242370),
  [33662445535](https://github.com/Takazudo/zudo-front-builder/actions/runs/33662445535)
  and
  [33688523915](https://github.com/Takazudo/zudo-front-builder/actions/runs/33688523915)
  plus local detector checks at `2026-09-03T04:16Z` and `2026-09-03T04:52Z`
  each returned rc 10, `errors: []`, and exactly one delta, `noyalib`
  `branch-advanced feat/v0.0.31` (`a1fb50918d8b7484b928118c9adffe852aacc5f1` →
  `b76f1aad8b773e1482f3cb12cdc760f86177bde6` by the last check), with no
  triage-severity delta on any candidate; that branch receives commits
  daily, so under single-tier classification the tracker could never close.
  The resolution was to reclassify, not to refresh — the baseline remains
  the `2026-09-02T21:08:22.477Z` observation, back-filled with a `yanked`
  field (schemaVersion 2: `["0.0.8-alpha-pre", "0.0.9"]` for
  `serde-saphyr`, `[]` for the other six), whose values are fully determined
  at that `checkedAt` because crates.io records `updated_at`
  `2025-11-24T13:24:30Z` and `2025-11-19T20:36:13Z` for the two yanks and no
  other tracked crate has a yanked version; the tracker (#2845) is closed by
  the first green exact-`main` run. The scheduled run fired at
  `2026-09-03T05:50:30Z` as
  [33720566956](https://github.com/Takazudo/zudo-front-builder/actions/runs/33720566956),
  still on the pre-fix single-tier `main` detector: rc 10, `errors: []`,
  the same `noyalib` `branch-advanced feat/v0.0.31` delta ending at
  `b76f1aad8b773e1482f3cb12cdc760f86177bde6`, and it appended one more rc-10
  comment on #2845 before this classification landed on `main`.

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

##### `noyalib 0.0.29` / `noyalib-serde-yaml 0.0.29` released differential evaluation (#2836) — RETAIN

The released evaluation started from zfb commit
`f9e8187e154a5df45ad5b506de72a76d7ab5e948`. Upstream's own shipped contract
predicted **17/18**: it pins `custom-explicit-tag` at Display column 16 while
zfb's immutable serde_yaml baseline pins column 8. The experiment independently
confirmed that prediction; the documented partial was not corrected or treated
as a match.

The trigger artifacts were re-queried at the start and before finalization.
[`noyalib 0.0.29`](https://crates.io/api/v1/crates/noyalib/0.0.29) was published
`2026-09-01T22:53:33Z`, is not yanked, is `MIT OR Apache-2.0`, requires Rust
1.86.0, and has sha256
`f102aea73ec3023f33cb904bcc59b8f2ac91bce8b77cbe16c14491d117c57b5e`.
Its `v0.0.29` tag resolves to
[`9dca232d`](https://github.com/sebastienrousseau/noyalib/commit/9dca232d7014b853a1c25e0768ddd12afa2873a2),
[PR 365](https://github.com/sebastienrousseau/noyalib/pull/365) merged at
`2026-09-01T22:36:28Z`, and the GitHub Release was published at
`2026-09-01T22:52:41Z`. A `feat/v0.0.30` branch exists.
[`noyalib-serde-yaml 0.0.29`](https://crates.io/api/v1/crates/noyalib-serde-yaml/0.0.29)
was published `2026-09-01T23:06:37Z`, is not yanked, and has sha256
`239d1cfb4eb2ccac255bdaa7583c97d48e28dfad3fefb33e06fbe6a89d4956a6`;
its tag resolves to
[`11cffda3`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/11cffda345303d9e19cadd3e297a4506818a4d01),
and it also has a `feat/v0.0.30` branch. No `0.0.30` publication existed for
either crate at either release-race check.

Only the exact crates.io archives above were inspected and built. Their bytes
matched the authorized checksums and the temporary `Cargo.lock` checksums.
The alias crate's published manifest has no patch, build script, or proc macro;
its sole normal dependency is exact `noyalib =0.0.29` with `std` and
`compat-serde-yaml`, and its library is a single public re-export. The noyalib
build script only declares two cfg names, runs `rustc --version`, and reads
`NOYALIB_COVERAGE`. No upstream example, script, or benchmark was executed.
There were no RUSTSEC advisories for either crate. The migration lock delta
observed exactly the predicted net-zero package count: remove `serde_yaml` and
`unsafe-libyaml`; add `noyalib` and `noyalib-serde-yaml`; all of noyalib's normal
dependencies were already locked.

Phase 1 exercised two paths. P1 changed only the root dependency to the exact
package alias, so all three production call sites compiled with **0 production
source lines changed**. P3 used exact `noyalib` with default features disabled
and features `std,compat-serde-yaml`, changed the frontmatter error and parse
paths (**2 rustfmt-formatted production lines**), compiled `zfb-content`, and
ran the same harness. Both paths produced this complete structured matrix;
`match` includes the success value or the complete error Display and optional
1-based line/column plus 0-based byte index:

| Category | Corpus case | P1 package alias | P3 direct shim |
| --- | --- | --- | --- |
| anchors-aliases | `anchors-and-aliases` | match | match |
| merge-keys | `merge-key-is-an-ordinary-json-key` | match | match |
| non-string-keys | `non-string-scalar-keys` | match | match |
| non-string-keys | `non-string-composite-key` | match | match |
| scalar-edge-cases | `yaml-11-boolean-spellings` | match | match |
| scalar-edge-cases | `octals-sexagesimals-and-numbers` | match | match |
| scalar-edge-cases | `null-and-date-scalars` | match | match |
| unicode-bom-crlf-emoji | `unicode-crlf-and-emoji` | match | match |
| malformed-input | `malformed-unicode-location` | match | match |
| malformed-input | `malformed-flow-sequence-at-eof` | match | match |
| malformed-input | `malformed-indentation` | match | match |
| explicit-tags | `built-in-explicit-tags` | match | match |
| explicit-tags | `custom-explicit-tag` | **mismatch** | **mismatch** |
| duplicate-keys | `duplicate-map-keys-last-wins` | match | match |
| non-finite-overflowing-numbers | `non-finite-and-overflowing-numbers` | match | match |
| non-finite-overflowing-numbers | `integer-boundaries` | match | match |
| non-finite-overflowing-numbers | `integer-overflow` | match | match |
| alias-anchor-resource-limits | `alias-anchor-repetition-limit` | match | match |

For the sole mismatch, the baseline is
`thing: invalid type: enum, expected any valid JSON value at line 1 column 8`
with location `{ line: 1, column: 8, index: 7 }`; both candidates observed the
same text ending in column 16 with
`{ line: 1, column: 16, index: 15 }`. Thus both paths were **17/18**.

Every protected assertion passed unchanged under P1:
`zfb-content/tests/error_messages.rs` (2/2), md-wasm's
`invalid_yaml_frontmatter_returns_frontmatter_diagnostic` with EOF source line
3, md-wasm's `frontmatter_diagnostics_use_original_source_utf16_columns` with
column 9, and zfb's `frontmatter_yaml_error_locates_within_user_file` unit test.
`cargo check -p zfb-content -p zfb -p zfb-md-wasm` also compiled the three
production consumers. P3's compile and harness were identical by construction.
Neither path required unsafe or FFI, OS-specific branches, polling, signal
supervision, generated Unicode tables, a corrective round, or more than 40
production lines.

The strict pre-declared gate makes any structural mismatch disqualifying, so
the terminal verdict is **RETAIN `serde_yaml`**. Phase 2 was correctly skipped:
the 18/18 prerequisite failed, independently of the machine's 16 GiB free disk
(which was also below Phase 2's 30 GiB minimum). No wasm build, md-wasm package
test, gzip-size delta, dependency/license analysis, or `cargo deny` candidate
run was performed.

The other-candidate status scan found no movement since the 2026-09-01
baseline: `serde_yaml_ng` remains 0.10.0 with canonical commit
[`36281029`](https://github.com/acatton/serde-yaml-ng/commit/3628102977f3ec9e02b95ef32fcec30b3df91390)
dated 2025-09-14; `serde_yml` remains 0.0.13 at
[`5caeeec0`](https://github.com/sebastienrousseau/serde_yml/commit/5caeeec0512296f985135502d36cd08e8ffb23d1)
dated 2026-05-28; `saphyr` remains 0.0.12 at
[`5c45acd3`](https://github.com/saphyr-rs/saphyr/commit/5c45acd365c711e3d92f0ed4a0ceabe349cde514)
dated 2026-08-18; `serde_norway` remains 0.9.42 at
[`1d37c159`](https://github.com/cafkafk/serde-norway/commit/1d37c159fc01c269a17ab72d021b271faf29472a)
dated 2024-12-21; and `serde-saphyr` remains 1.2.0 at
[`45042059`](https://github.com/bourumir-wyngs/serde-saphyr/commit/45042059e6905e833516f52d958cba4c16e8cedd)
dated 2026-08-30. The refreshed watcher snapshot records PR 365 as MERGED and
both evaluated crates at 0.0.29.

One isolated target, `/tmp/zfb-2836-cargo-target.ojB59v`, was used for every
candidate and restoration Cargo command. Before the experiment the filesystem
had 16 GiB free; the target reached 12 GiB at its peak. The experimental
manifests, lockfile, two frontmatter lines,
matrix printer, production consumers, immutable corpus/baseline, protected
assertions, shipped-size manifest, generated artifacts, and changelogs were
restored byte-for-byte. The target was validated, cleaned with Cargo, and
removed; the filesystem had 26 GiB free afterward. The next trigger is a
lockstep noyalib / noyalib-serde-yaml release whose
`crates/noyalib/tests/serde_yaml_contract.rs` pins `custom-explicit-tag` at
location `1:8:7` and Display column 8 via an exact assertion. #2755 remains
open. Reporting the diagnostic-anchor divergence upstream is recommended but
was not performed.

##### `noyalib 0.0.30` / `noyalib-serde-yaml 0.0.30` released differential evaluation (#2851) — MIGRATE

The released evaluation started from zfb commit
`66b77f22d673c078c3ec295b27e6730522869dc7`. Upstream's own shipped contract
predicted **18/18**: `crates/noyalib/tests/serde_yaml_contract.rs` at tag
`v0.0.30` asserts `custom-explicit-tag` as `"thing: invalid type: enum,
expected any valid JSON value at line 1 column 8"` with location
`Some((1, 8, 7))`, and its comment names this repository's #2755 as the
re-evaluation trigger for that exact pin. zfb's immutable serde_yaml baseline
carries the same text with `{ line: 1, column: 8, index: 7 }`. The experiment
independently confirmed the prediction; the prediction itself was never
treated as evidence.

The trigger artifacts were re-queried at the start (`2026-09-02T19:52Z`) and
again immediately before finalization (`2026-09-02T20:23:41Z`), and both
checks returned identical values.
[`noyalib 0.0.30`](https://crates.io/api/v1/crates/noyalib/0.0.30) was
published `2026-09-02T18:30:46Z`, is not yanked, is `MIT OR Apache-2.0`,
requires Rust 1.86.0, is 1,050,160 B, and has sha256
`23d48ffd97a6485043e2d05849188ba375a990a856b3607ce55e585a36ecfbb1`. Its
`v0.0.30` annotated tag dereferences to
[`e33ba3c9`](https://github.com/sebastienrousseau/noyalib/commit/e33ba3c90a02721388e974b458f07eeb0b40198a),
which is also `main`;
[PR 371](https://github.com/sebastienrousseau/noyalib/pull/371) merged at
`2026-09-02T18:11:27Z` and the GitHub Release was published at
`2026-09-02T18:29:57Z`. Its seven non-optional normal dependencies
(`hashbrown ^0.17`, `indexmap >=2,<3` with `serde`, `libm ^0.2`,
`memchr ^2.7`, `rustc-hash >=2,<3`, `serde_core ^1.0` with `alloc`, and
`smallvec ^1.13`) are unchanged from the #2836 record and were already locked.
[`noyalib-serde-yaml 0.0.30`](https://crates.io/api/v1/crates/noyalib-serde-yaml/0.0.30)
was published `2026-09-02T18:44:21Z`, is not yanked, is `MIT OR Apache-2.0`,
is 44,435 B, and has sha256
`d3b783463958e25c83a0aa93aa6adc9d632c85a1ef0aff4d81029871853631c7`; its
`v0.0.30` tag dereferences to
[`f12787c7`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/f12787c737bef8b579265fde52cd09696c60cfd6)
(also `main`, _"chore(release): v0.0.30 — lockstep with the noyalib core"_,
`2026-09-02T18:37:22Z`), its Release was published `2026-09-02T18:43:55Z`, and
a `release/v0.0.30` branch sits at the same SHA. Its sole normal dependency is
exact `noyalib =0.0.30` with `default-features = false` and features `std` and
`compat-serde-yaml`, and its library remains a single public re-export.
**Release-race row (a) therefore held at both checks**, so the package-alias
path on the sha256-pinned crates.io archives was primary and no capped-verdict
fallback was needed.

Only the exact crates.io archives above were inspected and built, and the
`Cargo.lock` checksums Cargo wrote matched the authorized sha256 pair
byte-for-byte in both paths. No upstream example, script, or benchmark was
executed, and the 2026-09-01 maintainer comment on #2755 was treated as data
under test rather than as instructions. `cargo deny check` reported no
advisory for either crate.

Phase 1 exercised two paths. P1 changed only the root workspace dependency to
the exact package alias `serde_yaml = { package = "noyalib-serde-yaml",
version = "=0.0.30" }`, leaving `crates/zfb-content/Cargo.toml` untouched, so
all three production call sites compiled with **0 production source lines
changed**. P3 declared exact `noyalib` in the workspace dependencies with default
features disabled and features `std,compat-serde-yaml`, switched
`crates/zfb-content/Cargo.toml` to `noyalib = { workspace = true }`, and
repointed the two `crates/zfb-content/src/frontmatter.rs` lines — the `#[from]`
error conversion at line 67 and the `from_str` call at line 254 — at
`noyalib::compat::serde_yaml`. That is **2 rustfmt-formatted production
lines** (`cargo fmt -p zfb-content -- --check` reported clean), two manifest
lines, and one test-only harness adapter line. `serde_yaml` is named in code by no other
crate: `crates/zfb/src/diagnostics.rs` and `crates/zfb-md-wasm/src/lib.rs`
mention it only in comments, so they consume the error type structurally and
needed no edit under either path. Both paths produced this complete structured
matrix, where `match` means the success value or the complete error Display
plus the optional 1-based line/column and 0-based byte index equalled the
immutable baseline:

| Category | Corpus case | P1 package alias | P3 direct shim |
| --- | --- | --- | --- |
| anchors-aliases | `anchors-and-aliases` | match | match |
| merge-keys | `merge-key-is-an-ordinary-json-key` | match | match |
| non-string-keys | `non-string-scalar-keys` | match | match |
| non-string-keys | `non-string-composite-key` | match | match |
| scalar-edge-cases | `yaml-11-boolean-spellings` | match | match |
| scalar-edge-cases | `octals-sexagesimals-and-numbers` | match | match |
| scalar-edge-cases | `null-and-date-scalars` | match | match |
| unicode-bom-crlf-emoji | `unicode-crlf-and-emoji` | match | match |
| malformed-input | `malformed-unicode-location` | match | match |
| malformed-input | `malformed-flow-sequence-at-eof` | match | match |
| malformed-input | `malformed-indentation` | match | match |
| explicit-tags | `built-in-explicit-tags` | match | match |
| explicit-tags | `custom-explicit-tag` | match | match |
| duplicate-keys | `duplicate-map-keys-last-wins` | match | match |
| non-finite-overflowing-numbers | `non-finite-and-overflowing-numbers` | match | match |
| non-finite-overflowing-numbers | `integer-boundaries` | match | match |
| non-finite-overflowing-numbers | `integer-overflow` | match | match |
| alias-anchor-resource-limits | `alias-anchor-repetition-limit` | match | match |

Both paths were therefore **18/18**. The single #2836 partial closed exactly
as upstream documented: `custom-explicit-tag` now anchors at
`{ line: 1, column: 8, index: 7 }` with Display column 8 instead of the
`1:16:15` and column 16 observed at 0.0.29. The corpus, the baseline fixture,
and the harness assertions were not edited, so the existing
`current_serde_yaml_matches_immutable_baseline` equality assertion passed
under both candidates on its own terms; the per-case matrix printer ran
alongside it and was removed before commit.

Every protected assertion passed unchanged under P1:
`crates/zfb-content/tests/error_messages.rs` (2/2),
`crates/zfb-md-wasm/tests/api.rs` `invalid_yaml_frontmatter_returns_frontmatter_diagnostic`
with its EOF source line 3 and column 1 pins,
`crates/zfb-md-wasm/tests/parse_to_ast.rs` `frontmatter_diagnostics_use_original_source_utf16_columns`
with column 9, and `crates/zfb/src/diagnostics.rs`'s
`diagnostics::tests::frontmatter_yaml_error_locates_within_user_file` unit test
run with `--no-default-features` to keep V8 out of the graph. The two md-wasm
suites passed in full at 32/32 and 25/25. `cargo check -p zfb-content
-p zfb-md-wasm` plus `cargo check -p zfb --no-default-features` compiled the
three production consumers with zero source changes. Neither path required
unsafe or FFI, OS-specific branches, polling, signal supervision, generated
Unicode tables, a corrective round, or more than 40 production lines, so the
standing abandon rule was not reached.

A local toolchain quirk is worth recording because it shaped how the commands
were invoked, not the result. On this Mac, Homebrew's `rustc 1.94.0` shadows
the rustup-managed `stable` (`rustc 1.96.0`) on `PATH` even under
`rustup run`, and the Homebrew build ships no `wasm32-unknown-unknown` std.
This is the same "Local-machine quirk" that `crates/zfb-md-wasm/SPIKE-FINDINGS.md`
documents and that `crates/zfb-md-wasm/npm/scripts/build.mjs` already works
around by prepending the active rustup toolchain's own bin directory. The P1
matrix was run on **both** toolchains and returned 18/18 on each, so the
contract result does not depend on the compiler; the P3 matrix was run on the
Homebrew toolchain only, and every Phase 2 command was pinned to the rustup
`stable` toolchain that `rust-toolchain.toml` selects.

Because the strict gate was satisfied — 18/18 on both paths, the protected set
unedited, and the abandon rule untouched — Phase 2 ran. The disk gate was
re-verified immediately beforehand at 157 GiB free, far above the 30 GiB
minimum. `cargo check --target wasm32-unknown-unknown -p zfb-md-wasm`
compiled under the alias, and `pnpm test:md-wasm` built all four wasm
artifacts and passed **219/219 tests across 12 files**.

The four gzip-9 artifact measurements are reported against two references,
because the committed manifest was produced on a different toolchain: a
`serde_yaml` control build on this same rustc 1.96.0 drifted from the
committed v2.14.3 manifest by +6,338 B on the default artifact's final wasm
before any dependency change. The **alias-attributable** delta is therefore
the third column against the second, not against the manifest.

| Artifact (gzip-9) | Committed v2.14.3 | `serde_yaml` control, rustc 1.96.0 | `noyalib-serde-yaml` alias | Alias delta vs control | Ceiling | Headroom |
| --- | --- | --- | --- | --- | --- | --- |
| default (root) | 1,502,166 | 1,511,297 | 1,520,675 | +9,378 | 1,600,000 | 79,325 |
| highlight-only | 816,904 | 822,539 | 822,446 | −93 | 880,000 | 57,554 |
| render-only | 1,069,796 | 1,074,557 | 1,093,417 | +18,860 | 1,100,000 | **6,583** |
| parse-only | 261,099 | 260,191 | 281,356 | +21,165 | 325,000 | 43,644 |

Final-wasm sizes tracked those figures, control to alias (default
3,359,843 → 3,391,961; highlight-only 1,533,462 → 1,533,104, the one artifact
that shrank; render-only 2,139,370 → 2,188,102; parse-only
644,715 → 695,713), the complete dist grew from 7,936,945 B to
8,068,435 B, and all eight glue and glue-gzip-9 figures were byte-identical
across all three columns. **Every artifact stays under its ceiling, so this is
not a blocker, but `render-only` clears its 1,100,000 B ceiling by only
6,583 B (0.6 %).** The migration topic must therefore re-measure on its own
build and treat the render ceiling as the tight constraint; the
`--update-manifest` evidence path was exercised for both the control and the
alias runs and the committed manifest was restored byte-for-byte afterwards
(sha256 `b49b061a4875ec3fc7a9ef99f53cda0a95aef97245433f2801264e33270a8d82`).

The lock delta under P1 was exactly the predicted net-zero package count:
`serde_yaml 0.9.34+deprecated` and `unsafe-libyaml 0.2.11` out, `noyalib
0.0.30` and `noyalib-serde-yaml 0.0.30` in, **597 packages before and after**.
P3, which omits the alias crate, gives 596. `cargo tree -e normal -i noyalib`
reports a single reverse path — `noyalib` ← `noyalib-serde-yaml` ←
`zfb-content` ← the rest of the workspace — the same shape `serde_yaml` had.
`cargo deny list` changed by one line in one category: `Apache-2.0` from 287
to 288, with `MIT` unchanged at 430 and all fourteen other license categories
unchanged, which is consistent with removing one dual-licensed and one
MIT-only crate and adding two dual-licensed ones. `cargo deny check` reported
`advisories ok, bans ok, licenses ok, sources ok` both before and after, and
**no new exception was required**: `deny.toml` contains no `serde_yaml`,
`unsafe-libyaml`, or `noyalib` entry.

With row (a) confirmed at the final re-check, 18/18 on both paths, the
protected set unedited, and Phase 2 green, the terminal verdict is
**MIGRATE**. The migration itself is a separate, separately reviewable topic;
this section records only the evidence for it.

The other-candidate status scan found no movement since the 2026-09-02
baseline: `serde_yaml_ng` remains 0.10.0 with canonical commit
[`36281029`](https://github.com/acatton/serde-yaml-ng/commit/3628102977f3ec9e02b95ef32fcec30b3df91390)
dated 2025-09-14; `serde_yml` remains 0.0.13 at
[`5caeeec0`](https://github.com/sebastienrousseau/serde_yml/commit/5caeeec0512296f985135502d36cd08e8ffb23d1)
dated 2026-05-28, with its repository still archived; `saphyr` remains 0.0.12
at [`5c45acd3`](https://github.com/saphyr-rs/saphyr/commit/5c45acd365c711e3d92f0ed4a0ceabe349cde514)
dated 2026-08-18; `serde_norway` remains 0.9.42 at
[`1d37c159`](https://github.com/cafkafk/serde-norway/commit/1d37c159fc01c269a17ab72d021b271faf29472a)
dated 2024-12-21; and `serde-saphyr` remains 1.2.0 at
[`45042059`](https://github.com/bourumir-wyngs/serde-saphyr/commit/45042059e6905e833516f52d958cba4c16e8cedd)
dated 2026-08-30. None of the five is a trigger. `noyalib-serde-yaml` is the
sixth tracked entry and moved to 0.0.30 at
[`f12787c7`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/f12787c737bef8b579265fde52cd09696c60cfd6)
dated 2026-09-02, which is the lockstep alias evaluated above rather than a
separate candidate. No build was performed for any of the six.

One isolated target, `/tmp/zfb-2851-cargo-target.u9Wcq9`, was used for every
candidate command, and every cargo and wasm command was serialized. The
filesystem had 164 GiB free at the start, so the epic pre-flight had already
reclaimed well past the gate and no deletion was performed by this topic; it
reached a 153 GiB low point during Phase 2 and returned to 156 GiB once the
target was validated and removed. The isolated target peaked at **8.3 GiB**,
below the 12 GiB #2836 measured, which the next round can use to right-size
the gate. The experimental manifests, lockfile, two frontmatter lines, matrix
printer, harness adapter, production consumers, immutable corpus and baseline,
protected assertions, shipped-size manifest, generated artifacts, and every
changelog page were restored byte-for-byte: `git status --porcelain` was empty
apart from this file, and a sha256 compare over 178 files — the 13 manifests,
fixtures, production sources and protected tests plus all 165 changelog pages
— reported zero mismatches. On the restored tree `cargo nextest run
-p zfb-content --test yaml_differential_harness` passed 4/4 and
`pnpm test:workspace` passed 823 tests across 7 packages with exit code 0.

This section used `Refs #2755` and closed nothing — consistent with every PR
body in the retirement epic, which never writes `Closes #2755`. **Update
(2026-09-03):** #2755 was closed with a terminal comment once PR #2854
merged this migration; a follow-up epic then retired the standing-trigger
wording across the detector, its workflow, and this ledger (see the "YAML
candidate watch" lane paragraph above for the current protocol). No
upstream report was warranted, because the divergence #2836 recorded is the
one upstream fixed in this release. Acknowledging that upstream is an
outward-facing action and remains the owner's call; it was not performed.

##### `noyalib 0.0.31` / `noyalib-serde-yaml 0.0.31` released differential evaluation (#2873) — MIGRATE

This round re-runs the owner's evaluation protocol against the adopted pair's
next lockstep release. It starts from zfb commit
`d1152f117824126f315ee68ce9ea33456dfc660a`. Upstream's own shipped contract
predicted **18/18**: `crates/noyalib/tests/serde_yaml_contract.rs` at tag
`v0.0.31` still asserts `custom-explicit-tag` as `"thing: invalid type: enum,
expected any valid JSON value at line 1 column 8"` with location
`Some((1, 8, 7))`, byte-identical to the pin zfb's immutable baseline requires
and to the text this repository adopted at 0.0.30. The experiment
independently confirmed the prediction; the prediction itself was never
treated as evidence.

The trigger artifacts were re-queried at the start of this topic, alongside
the `2026-09-03T11:49:34.325Z` detector observation recorded above, and again
immediately before finalization (`2026-09-03T12:48:09Z`); both checks returned
identical values.
[`noyalib 0.0.31`](https://crates.io/api/v1/crates/noyalib/0.0.31) was
published `2026-09-03T09:20:10.855620Z`, is not yanked, is
`MIT OR Apache-2.0`, requires Rust 1.86.0, is 1,058,758 B, and has sha256
`6c34297b0e8a3fc5a5245f7ea28e50fc96d290b28e3d0d0da8e7c14235ec33b0`. Its
`v0.0.31` annotated tag (tagger date `2026-09-03T09:02:27Z`, tag object
`ee9e2176ded3b7e348ef46f42577f26525037d35`) dereferences to
[`f57afdd8`](https://github.com/sebastienrousseau/noyalib/commit/f57afdd8d2ef2645578a80b089b3485bcc72b633),
which is also `main`, and the GitHub Release was published at
`2026-09-03T09:19:12Z`. Its seven non-optional normal dependencies
(`hashbrown ^0.17`, `indexmap >=2,<3` with `serde`, `libm ^0.2`,
`memchr ^2.7`, `rustc-hash >=2,<3`, `serde_core ^1.0` with `alloc`, and
`smallvec ^1.13`) are unchanged from the #2851 record and were already locked,
so the lock delta could not reach beyond the two adopted entries.
[`noyalib-serde-yaml 0.0.31`](https://crates.io/api/v1/crates/noyalib-serde-yaml/0.0.31)
was published `2026-09-03T09:39:48.290605Z`, is not yanked, is
`MIT OR Apache-2.0`, requires Rust 1.86.0, is 46,937 B, and has sha256
`8eaae0a5d646674f179b3ae62539049568ffa065aaadb89a7135ef5e1c1c274e`; its
`v0.0.31` annotated tag (tagger date `2026-09-03T09:36:39Z`, tag object
`fbbe2798e3ad8535f8c7eb2654e599568c8501ed`) dereferences to
[`27fbdd9e`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/27fbdd9e54c982df09d4ffda6de0267a36c9ab4d)
(also `main`, _"chore(release): v0.0.31 — lockstep with the noyalib core"_),
and its Release was published `2026-09-03T09:39:23Z`. Its sole normal
dependency is exact `noyalib =0.0.31` with `default-features = false` and
features `std` and `compat-serde-yaml`, and its `src/lib.rs`, read at the tag,
remains exactly `#![forbid(unsafe_code)]` plus
`pub use noyalib::compat::serde_yaml::*;` with no `build.rs` — a single public
re-export. Neither repository had an open pull request at either check,
so `pendingReleasePr` correctly stays `null`.

**Release-race row (a) therefore held at both checks** — both crates on
crates.io, un-yanked, checksums equal to the authorized sha256 pair, both tags
resolving to the SHAs above, and the alias pinning exact `noyalib =0.0.31` —
so the package-alias path on the sha256-pinned crates.io archives was primary
and no capped-verdict fallback (rows (b), (b′), (c)) was needed.

Only the exact crates.io archives above were inspected and built, and the
`Cargo.lock` checksums Cargo wrote were compared against the authorized
sha256 pair **before** any build and matched byte-for-byte. No upstream
example, script, or benchmark was executed. Upstream's `v0.0.31` release notes
also carry `test(resilience)!: harness adoptions from the zfb evaluation
stack`, which is test-only and was read, not run.

Phase 1 exercised two paths. P1 changed only the root workspace dependency's
version to the exact package alias `serde_yaml = { package =
"noyalib-serde-yaml", version = "=0.0.31" }`, leaving
`crates/zfb-content/Cargo.toml` untouched, so all three production call sites
compiled with **0 production source lines changed**. P3 declared exact
`noyalib` in the workspace dependencies with default features disabled and
features `std,compat-serde-yaml`, switched `crates/zfb-content/Cargo.toml` to
`noyalib = { workspace = true }`, and repointed the two
`crates/zfb-content/src/frontmatter.rs` lines — the `#[from]` error conversion
at line 67 and the `from_str` call at line 254 — at
`noyalib::compat::serde_yaml`. That is **2 rustfmt-formatted production
lines** (`cargo fmt -p zfb-content -- --check` reported clean), two manifest
lines, and one test-only harness adapter line. Both paths produced this
complete structured matrix, where `match` means the success value or the
complete error Display plus the optional 1-based line/column and 0-based byte
index equalled the immutable baseline:

| Category | Corpus case | P1 package alias | P3 direct shim |
| --- | --- | --- | --- |
| anchors-aliases | `anchors-and-aliases` | match | match |
| merge-keys | `merge-key-is-an-ordinary-json-key` | match | match |
| non-string-keys | `non-string-scalar-keys` | match | match |
| non-string-keys | `non-string-composite-key` | match | match |
| scalar-edge-cases | `yaml-11-boolean-spellings` | match | match |
| scalar-edge-cases | `octals-sexagesimals-and-numbers` | match | match |
| scalar-edge-cases | `null-and-date-scalars` | match | match |
| unicode-bom-crlf-emoji | `unicode-crlf-and-emoji` | match | match |
| malformed-input | `malformed-unicode-location` | match | match |
| malformed-input | `malformed-flow-sequence-at-eof` | match | match |
| malformed-input | `malformed-indentation` | match | match |
| explicit-tags | `built-in-explicit-tags` | match | match |
| explicit-tags | `custom-explicit-tag` | match | match |
| duplicate-keys | `duplicate-map-keys-last-wins` | match | match |
| non-finite-overflowing-numbers | `non-finite-and-overflowing-numbers` | match | match |
| non-finite-overflowing-numbers | `integer-boundaries` | match | match |
| non-finite-overflowing-numbers | `integer-overflow` | match | match |
| alias-anchor-resource-limits | `alias-anchor-repetition-limit` | match | match |

Both paths were therefore **18/18**, and the two paths' per-case observations
were byte-identical to each other as well as to the baseline — the
identical-by-construction claim held. `custom-explicit-tag` stays at
`{ line: 1, column: 8, index: 7 }` with Display column 8, so the pin #2755
demanded and 0.0.30 delivered survives the upgrade. The corpus, the baseline
fixture, and the harness assertions were not edited, so the existing
`current_serde_yaml_matches_immutable_baseline` equality assertion passed
under both candidates on its own terms; the per-case matrix printer ran
alongside it and was removed before commit. The harness's
`CURRENT_ADAPTER_NAME` label still reads `0.0.30` by design: it is asserted
equal to the baseline fixture's own `adapter` field, so the two must agree
with each other rather than with the resolved crate version. Editing it
without editing the immutable fixture would have broken a passing assertion
for no evidentiary gain, so it was deliberately left alone. (The pin-bump
topic [#2875](https://github.com/Takazudo/zudo-front-builder/issues/2875)
later relabelled both strings to `0.0.31` together; the 18 cases themselves
are unchanged.)

Every protected assertion passed unchanged under P1:
`crates/zfb-content/tests/error_messages.rs` (2/2),
`crates/zfb-md-wasm/tests/api.rs` `invalid_yaml_frontmatter_returns_frontmatter_diagnostic`
with its EOF source line 3 and column 1 pins,
`crates/zfb-md-wasm/tests/parse_to_ast.rs` `frontmatter_diagnostics_use_original_source_utf16_columns`
with column 9, and `crates/zfb/src/diagnostics.rs`'s
`diagnostics::tests::frontmatter_yaml_error_locates_within_user_file` unit test
run with `--no-default-features` to keep V8 out of the graph. `cargo check
-p zfb-content` and `cargo check -p zfb-md-wasm`, plus `cargo check -p zfb
--no-default-features`, compiled the three production consumers with zero
source changes; the three checks were run separately rather than as one
multi-package invocation so that feature unification could not pull
`embed_v8` back into the graph. Neither path required unsafe or FFI,
OS-specific branches, polling, signal supervision, generated Unicode tables, a
corrective round, or more than 40 production lines, so the standing abandon
rule was not reached.

The local toolchain quirk #2851 recorded is unchanged and again shaped how the
commands were invoked, not the result: Homebrew's `rustc 1.94.0` shadows the
rustup-managed `stable` (`rustc 1.96.0`) on `PATH`, and the Homebrew build
ships no `wasm32-unknown-unknown` std. Phase 1 ran on the Homebrew toolchain only —
a narrower check than #2851, which ran its P1 matrix on both toolchains and
got 18/18 on each; the cross-toolchain evidence this round is indirect,
namely that Phase 2 compiled `zfb-content` against `noyalib 0.0.31` under
rustc 1.96.0 and the md-wasm frontmatter-diagnostic tests passed there.
Every Phase 2 command was pinned to the rustup `stable` toolchain that
`rust-toolchain.toml` selects, by prepending that toolchain's own bin
directory — the same workaround `crates/zfb-md-wasm/npm/scripts/build.mjs`
already applies internally. One additional provisioning step was needed this
round: the pinned `wasm-bindgen` CLI 0.2.121 was absent from this machine, so
it was installed with `cargo install wasm-bindgen-cli --version 0.2.121
--locked --root /tmp/eval0031-tools` — the exact command the build script's
own error message prints — contained outside the user's cargo bin directory
and removed afterwards. `wasm-opt` came from the repo's pinned `binaryen`
130.0.0 devDependency and needed no provisioning.

Because the strict gate was satisfied — 18/18 on both paths, the protected set
unedited, and the abandon rule untouched — Phase 2 ran. The disk gate was
re-verified immediately beforehand at 63 GiB free, above the 30 GiB minimum.
`cargo check --target wasm32-unknown-unknown -p zfb-md-wasm` compiled under
the alias, and `pnpm test:md-wasm` built all four wasm artifacts and passed
**219/219 tests across 12 files**.

All sixteen measured manifest fields were captured on both sides of the bump
using the same rustc 1.96.0, because the committed manifest was produced by
CI on a different toolchain and cannot be compared directly. The `0.0.30`
column is a control build of the currently committed pin; the delta column is
therefore the **bump-attributable** figure. Both columns were produced through
the documented `--update-manifest` evidence path, and the committed manifest
was restored byte-for-byte afterwards (sha256
`eccae493efaebbe635694e95bd6bef250f8ffaafbab84a339e1f9a34d1df667c`).

| Artifact | Field | `0.0.30` control | `0.0.31` | Delta | Committed (CI, v2.14.3) |
| --- | --- | --- | --- | --- | --- |
| root | `finalWasm` | 3,391,961 | 3,391,945 | −16 | 3,397,943 |
| root | `gzip9` | 1,520,675 | 1,520,733 | +58 | 1,515,424 |
| root | `glue` | 14,998 | 14,998 | 0 | 14,998 |
| root | `glueGzip9` | 4,199 | 4,199 | 0 | 4,199 |
| highlight | `finalWasm` | 1,533,104 | 1,533,152 | +48 | 1,536,686 |
| highlight | `gzip9` | 822,446 | 822,486 | +40 | 817,181 |
| highlight | `glue` | 8,758 | 8,758 | 0 | 8,758 |
| highlight | `glueGzip9` | 2,637 | 2,637 | 0 | 2,637 |
| render | `finalWasm` | 2,188,102 | 2,187,642 | −460 | 2,185,000 |
| render | `gzip9` | 1,093,417 | 1,093,241 | −176 | 1,087,865 |
| render | `glue` | 8,772 | 8,772 | 0 | 8,772 |
| render | `glueGzip9` | 2,661 | 2,661 | 0 | 2,661 |
| parse | `finalWasm` | 695,713 | 695,642 | −71 | 694,299 |
| parse | `gzip9` | 281,356 | 281,325 | −31 | 281,759 |
| parse | `glue` | 11,159 | 11,159 | 0 | 11,159 |
| parse | `glueGzip9` | 3,797 | 3,797 | 0 | 3,797 |

Four things follow from that table. First, the `0.0.30` control reproduces the
#2851 alias column **byte-for-byte on all eight wasm figures**, which
independently corroborates both rounds' measurement method. Second, **the bump
does move measured wasm bytes**, so the delegated decision recorded in the
epic resolves in the affirmative: the `cst` module changes are compiled under
the `std` feature the alias enables even though zfb never calls them, and the
migration topic must refresh the manifest rather than assume a no-op. Third,
the deltas are tiny and run in both directions (−460 B to +58 B), and all
eight glue and glue-gzip-9 figures are byte-identical across all three
columns, which is consistent with a change confined to compiled Rust rather
than to bindgen output. Fourth, and most consequential for the migration
topic: **the CI-versus-local toolchain gap is an order of magnitude larger
than the bump itself** — up to 5,982 B on the committed pin's own artifacts —
so a locally refreshed manifest will not match CI's bytes, and the manager
must expect to align the manifest from CI's own numbers, exactly as the
`a6509185` precedent in #2854 did.

Every artifact stays under its ceiling, and the ceiling that #2851 flagged as
tight relaxes very slightly: `render-only` gzip-9 clears its 1,100,000 B
ceiling by **6,759 B** at 0.0.31 versus 6,583 B on the control, a 176 B
improvement. Headroom on the other three is root 79,267 B, highlight-only
57,514 B, and parse-only 43,675 B. Those are local figures; the headroom that
actually gates CI is computed from the committed manifest and is wider on
three of the four — root 84,576 B, highlight-only 62,819 B, render-only
12,135 B, parse-only 43,241 B. The complete dist shrank from 8,068,435 B to
8,067,936 B.

The lock delta under P1 was exactly the predicted minimum: `noyalib` and
`noyalib-serde-yaml` each moved `0.0.30` → `0.0.31` with the two authorized
checksums replacing the previous pair, both dependency lists unchanged, and
**597 packages before and after**; no other line of `Cargo.lock` moved. P3,
which omits the alias crate, gives 596. `cargo tree -e normal -i noyalib`
reports a single reverse path — `noyalib` ← `noyalib-serde-yaml` ←
`zfb-content` ← the rest of the workspace — unchanged in shape from 0.0.30.
`cargo deny list` differs in exactly four strings, the two crates' versions in
each of the `Apache-2.0` and `MIT` categories, with every license category
count identical (`Apache-2.0` 288, `MIT` 430), which is what a dual-licensed
in-place version bump must look like. `cargo deny check` reported
`advisories ok, bans ok, licenses ok, sources ok` with exit code 0 both before
and after, with the same 13 pre-existing wildcard warnings about internal
`path` dependencies, and **no new exception was required**: `deny.toml`
contains no `noyalib` entry.

With row (a) confirmed at the final re-check, 18/18 on both paths, the
protected set unedited, and Phase 2 green, the terminal verdict is
**MIGRATE**. The pin bump itself is a separate, separately reviewable topic
([#2875](https://github.com/Takazudo/zudo-front-builder/issues/2875)); this
section records only the evidence for it, and performs no baseline refresh and
no detector edit.

The other-candidate status scan found no movement since the 2026-09-02
baseline: `serde_yaml_ng` remains 0.10.0 with canonical commit
[`36281029`](https://github.com/acatton/serde-yaml-ng/commit/3628102977f3ec9e02b95ef32fcec30b3df91390)
dated 2025-09-14; `serde_yml` remains 0.0.13 at
[`5caeeec0`](https://github.com/sebastienrousseau/serde_yml/commit/5caeeec0512296f985135502d36cd08e8ffb23d1)
dated 2026-05-28; `saphyr` remains 0.0.12 at
[`5c45acd3`](https://github.com/saphyr-rs/saphyr/commit/5c45acd365c711e3d92f0ed4a0ceabe349cde514)
dated 2026-08-18; `serde_norway` remains 0.9.42 at
[`1d37c159`](https://github.com/cafkafk/serde-norway/commit/1d37c159fc01c269a17ab72d021b271faf29472a)
dated 2024-12-21; and `serde-saphyr` remains 1.2.0 at
[`45042059`](https://github.com/bourumir-wyngs/serde-saphyr/commit/45042059e6905e833516f52d958cba4c16e8cedd)
dated 2026-08-30. None of the five is a trigger. `noyalib-serde-yaml` is the
sixth tracked entry and moved to 0.0.31 at
[`27fbdd9e`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/27fbdd9e54c982df09d4ffda6de0267a36c9ab4d)
dated 2026-09-03, which is the lockstep alias evaluated above rather than a
separate candidate. No build was performed for any of the six.

One isolated target, `/tmp/eval0031-target.EqoBF6`, was used for every
candidate command across both evaluation paths and Phase 2, and every cargo
and wasm command was serialized. The filesystem had 70 GiB free at the start
with the main checkout's `./target` at 10 GB and no `cargo` or `rustc` process
running, so the 30 GiB gate already held and **no pre-flight deletion was
performed**; it was re-verified at 63 GiB immediately before Phase 2, reached
a 61 GiB low point during it, and returned to 63 GiB once the temporary
directories were validated and removed. The isolated target peaked at **7.1 GiB**, below the
8.3 GiB #2851 measured and well below the 12 GiB #2836 needed; the separate
`wasm-bindgen` CLI build used a further 375 MiB, and both were removed at the
end. The experimental manifests, lockfile, two frontmatter lines, matrix
printer, harness adapter, production consumers, immutable corpus and baseline,
protected assertions, shipped-size manifest, generated artifacts, and every
changelog page were restored byte-for-byte. `git status --porcelain` was empty
apart from this file, which is itself the proof for every tracked file
including all changelog pages, and an explicit sha256 compare over the 13
manifests, fixtures, production sources, protected tests and the shipped-size
manifest reported zero mismatches against the values captured before any
edit. On the restored tree `cargo nextest run -p zfb-content
--test yaml_differential_harness` passed 4/4 and `pnpm test:workspace` passed
861 tests across 58 files in 7 packages with exit code 0.

This section used `Refs #2870` and closed nothing. **No upstream report is
warranted**: 0.0.31 preserves every pin zfb depends on and introduces no
divergence to report. Acknowledging the release upstream would be an
outward-facing action and remains the owner's call; it was not performed.

##### `noyalib 0.0.43` / `noyalib-serde-yaml 0.0.43` released differential evaluation (#2988) — MIGRATE

This round re-runs the owner's evaluation protocol against the adopted pair's
current lockstep release, eleven versions ahead of the committed `0.0.31` pin.
It starts from zfb commit
`1c2bfade4702d19faa43ab5bb95e169ee9877051`. Upstream's own shipped contract
again predicted **18/18**: `crates/noyalib/tests/serde_yaml_contract.rs` at tag
`v0.0.43` still asserts `custom-explicit-tag` as `"thing: invalid type: enum,
expected any valid JSON value at line 1 column 8"` with location
`Some((1, 8, 7))`, byte-identical to the pin zfb's immutable baseline requires
and to the text this repository has carried since 0.0.30, and its comment
still names `Takazudo/zudo-front-builder#2755` as the reason that pin exists.
The experiment independently confirmed the prediction; the prediction itself
was never treated as evidence.

The trigger artifacts were queried at the start of this topic
(`2026-09-13T20:16:04Z`) and again immediately before finalization
(`2026-09-13T21:02:54Z`); both checks returned identical values.
[`noyalib 0.0.43`](https://crates.io/api/v1/crates/noyalib/0.0.43) was
published `2026-09-08T13:19:16.276492Z`, is not yanked, is
`MIT OR Apache-2.0`, requires Rust 1.86.0, is 1,118,727 B, and has sha256
`55f03682003f288e23f94d8f6222de0dcf15c77329b5e42b2f37421243c1d373`. Its
`v0.0.43` annotated tag (tagger date `2026-09-08T13:00:22Z`, tag object
`59d6140a9c942c53cbe5ed7b708082a8f30e1b39`) dereferences to
[`a939aae0`](https://github.com/sebastienrousseau/noyalib/commit/a939aae03cb142f09ea5b9476e70218065e8ca79),
which is also `main`, and the GitHub Release was published at
`2026-09-08T13:18:24Z`. Its seven non-optional normal dependencies
(`hashbrown ^0.17`, `indexmap >=2,<3` with `serde`, `libm ^0.2`,
`memchr ^2.7`, `rustc-hash >=2,<3`, `serde_core ^1.0` with `alloc`, and
`smallvec ^1.13`) are unchanged from the #2851 and #2873 records and were
already locked. Its **optional** set has grown since 0.0.31 — it now offers
`arbitrary`, `ariadne`, `bytes`, `figment`, `garde`, `itoa`, `jsonschema`,
`miette`, `rayon`, `ryu`, `schemars`, `serde-saphyr`, `serde_ignored`,
`serde_json`, `sval`, `tokio`, `tokio-util`, and `validator` — but the alias
enables only `std` and `compat-serde-yaml` with default features off, so none
of them can enter the graph, and the lock delta could not reach beyond the two
adopted entries. The measured lock delta below confirms that it did not.
[`noyalib-serde-yaml 0.0.43`](https://crates.io/api/v1/crates/noyalib-serde-yaml/0.0.43)
was published `2026-09-08T13:28:07.717885Z`, is not yanked, is
`MIT OR Apache-2.0`, requires Rust 1.86.0, is 64,754 B, and has sha256
`20846fcf3ba8bf676a2d85708086f7ab6b45c40eb3682a956ffccd7199ded7b2`; its
`v0.0.43` annotated tag (tagger date `2026-09-08T13:24:08Z`, tag object
`90932b7ac3cbeca5d2ac5efbc5876f0c896b36a1`) dereferences to
[`02bc4628`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/02bc46287a04e2e66bed90c4836b6afb8af359d0)
(also `main`), and its Release was published `2026-09-08T13:27:42Z`. Both
Release bodies publish the crate checksum, and both match the crates.io
record above. Its sole normal dependency is exact `noyalib =0.0.43` with
`default-features = false` and features `std` and `compat-serde-yaml`, and
its `src/lib.rs`, read at the tag, remains exactly `#![forbid(unsafe_code)]`
plus `#![deny(missing_docs)]` plus `pub use noyalib::compat::serde_yaml::*;`
with no `build.rs` — a single public re-export. The alias repository had no
open pull request at either check. `sebastienrousseau/noyalib` carried four
open pull requests (#422, #424, #426, #427), every one of them an ordinary
`fix/…` or `feat/…` CST branch rather than a release PR, so
`CANDIDATE_CONFIG.noyalib.pendingReleasePr` correctly stays `null` and needs
no edit.

**Release-race row (a) therefore held at both checks** — both crates on
crates.io, un-yanked, checksums equal to the authorized sha256 pair, both tags
resolving to the SHAs above, `0.0.43` the newest published version of each,
and the alias pinning exact `noyalib =0.0.43` — so the package-alias path on
the sha256-pinned crates.io archives was primary and no capped-verdict
fallback (rows (b), (b′), (c)) was needed.

Eleven versions were passed over between the committed pin and this one. Each
is published and un-yanked, and none was evaluated:

| Version | `noyalib` published | `noyalib-serde-yaml` published | Yanked |
| --- | --- | --- | --- |
| 0.0.32 | `2026-09-03T20:50:58.267332Z` | `2026-09-03T21:21:36.356265Z` | no |
| 0.0.33 | `2026-09-05T16:07:24.925613Z` | `2026-09-05T16:21:25.749147Z` | no |
| 0.0.34 | `2026-09-05T23:39:14.257880Z` | `2026-09-05T23:49:32.996400Z` | no |
| 0.0.35 | `2026-09-06T12:58:27.899124Z` | `2026-09-06T13:18:09.240533Z` | no |
| 0.0.36 | `2026-09-06T15:25:46.474273Z` | `2026-09-06T15:33:11.373469Z` | no |
| 0.0.37 | `2026-09-06T16:57:09.865038Z` | `2026-09-06T17:05:10.941003Z` | no |
| 0.0.38 | `2026-09-06T19:51:54.777212Z` | `2026-09-06T19:59:34.372356Z` | no |
| 0.0.39 | `2026-09-07T00:59:00.662740Z` | `2026-09-07T01:13:01.797711Z` | no |
| 0.0.40 | `2026-09-07T12:16:58.135816Z` | `2026-09-07T12:25:45.015989Z` | no |
| 0.0.41 | `2026-09-07T14:42:14.516968Z` | `2026-09-07T14:53:12.668197Z` | no |
| 0.0.42 | `2026-09-08T07:24:12.271721Z` | `2026-09-08T07:37:45.289650Z` | no |

The lockstep invariant held across all eleven: every `noyalib` release was
followed by an identically numbered alias release within roughly half an hour,
and neither crate ever published a version the other lacks. This round
evaluates only `0.0.43`; the eleven intermediate pairs are recorded so the
gap is visible, not because any of them was built.

Only the exact crates.io archives above were inspected and built, and the
`Cargo.lock` checksums Cargo wrote were compared against the authorized
sha256 pair **before** any build and matched byte-for-byte. No upstream
example, script, or benchmark was executed; the two release bodies were read,
not run.

Phase 1 exercised path **P1 only**. P1 changed only the root workspace
dependency's version to the exact package alias `serde_yaml = { package =
"noyalib-serde-yaml", version = "=0.0.43" }`, leaving
`crates/zfb-content/Cargo.toml` untouched, so all three production call sites
compiled with **0 production source lines changed**. P1 came back 18/18, so
the P3 direct-shim path — which the protocol reserves for a P1 that falls
short — was not run, and the `P3` column the #2873 table carries has no
counterpart here. `match` below means the success value or the complete error
Display plus the optional 1-based line/column and 0-based byte index equalled
the immutable baseline:

| Category | Corpus case | P1 package alias |
| --- | --- | --- |
| anchors-aliases | `anchors-and-aliases` | match |
| merge-keys | `merge-key-is-an-ordinary-json-key` | match |
| non-string-keys | `non-string-scalar-keys` | match |
| non-string-keys | `non-string-composite-key` | match |
| scalar-edge-cases | `yaml-11-boolean-spellings` | match |
| scalar-edge-cases | `octals-sexagesimals-and-numbers` | match |
| scalar-edge-cases | `null-and-date-scalars` | match |
| unicode-bom-crlf-emoji | `unicode-crlf-and-emoji` | match |
| malformed-input | `malformed-unicode-location` | match |
| malformed-input | `malformed-flow-sequence-at-eof` | match |
| malformed-input | `malformed-indentation` | match |
| explicit-tags | `built-in-explicit-tags` | match |
| explicit-tags | `custom-explicit-tag` | match |
| duplicate-keys | `duplicate-map-keys-last-wins` | match |
| non-finite-overflowing-numbers | `non-finite-and-overflowing-numbers` | match |
| non-finite-overflowing-numbers | `integer-boundaries` | match |
| non-finite-overflowing-numbers | `integer-overflow` | match |
| alias-anchor-resource-limits | `alias-anchor-repetition-limit` | match |

P1 was therefore **18/18**. `custom-explicit-tag` stays at
`{ line: 1, column: 8, index: 7 }` with Display column 8, so the pin #2755
demanded and 0.0.30 delivered survives an eleven-version jump. The corpus, the
baseline fixture, and the harness assertions were not edited, so the existing
`current_serde_yaml_matches_immutable_baseline` equality assertion passed under
the candidate on its own terms; the per-case matrix printer ran alongside it
and was removed before commit. The harness's `CURRENT_ADAPTER_NAME` label
still reads `0.0.31` by design: it is asserted equal to the baseline fixture's
own `adapter` field, so the two must agree with each other rather than with
the resolved crate version. Editing it without editing the immutable fixture
would have broken a passing assertion for no evidentiary gain, so it was
deliberately left alone; relabelling both strings together belongs to the pin
bump, exactly as #2875 did for 0.0.31.

Every protected assertion passed unchanged under P1:
`crates/zfb-content/tests/error_messages.rs` (2/2),
`crates/zfb-md-wasm/tests/api.rs` `invalid_yaml_frontmatter_returns_frontmatter_diagnostic`
with its EOF source line 3 and column 1 pins,
`crates/zfb-md-wasm/tests/parse_to_ast.rs` `frontmatter_diagnostics_use_original_source_utf16_columns`
with column 9, and `crates/zfb/src/diagnostics.rs`'s
`diagnostics::tests::frontmatter_yaml_error_locates_within_user_file` unit test
run with `--no-default-features` to keep V8 out of the graph. `cargo check
-p zfb-content` and `cargo check -p zfb-md-wasm`, plus `cargo check -p zfb
--no-default-features`, compiled the three production consumers with zero
source changes; the three checks were run separately rather than as one
multi-package invocation so that feature unification could not pull
`embed_v8` back into the graph. The 28 warnings `cargo check -p zfb
--no-default-features` emits are the pre-existing feature-gated dead-code
warnings of that configuration and are unrelated to the candidate. Neither
unsafe nor FFI, OS-specific branches, polling, signal supervision, generated
Unicode tables, a corrective round, or more than 40 production lines were
required, so the standing abandon rule was not reached.

The local toolchain quirk #2851 recorded is unchanged: Homebrew's
`rustc 1.94.0` shadows the rustup-managed `stable` on `PATH`, and the Homebrew
build ships no `wasm32-unknown-unknown` std. This round differs from #2873 in
how that was handled — **every** command, Phase 1 and Phase 2 alike, was
pinned to the rustup `stable` toolchain that `rust-toolchain.toml` selects by
prepending that toolchain's own bin directory, the same workaround
`crates/zfb-md-wasm/npm/scripts/build.mjs` applies internally. One toolchain
throughout keeps the shared target directory warm and makes the Phase 1 and
Phase 2 evidence directly comparable, but it also means this round carries
**no Homebrew-toolchain observation at all**: #2851 ran its P1 matrix on both
toolchains and got 18/18 on each, #2873 ran Phase 1 on Homebrew only, and this
round ran everything on rustc 1.96.0. Cross-toolchain agreement at 0.0.43 is
therefore untested and is this round's named blind spot. The exact versions
used were **`rustc 1.96.0 (ac68faa20 2026-05-25)`** with
`cargo 1.96.0 (30a34c682 2026-05-25)`, **`wasm-bindgen 0.2.121`**, and
**`wasm-opt version 130 (version_130)`**. The pinned `wasm-bindgen` CLI 0.2.121
was again absent from this machine, so it was installed with
`cargo install wasm-bindgen-cli --version 0.2.121 --locked --root
/tmp/eval0043-tools` — the exact command the build script's own error message
prints — contained outside the user's cargo bin directory and removed
afterwards. `wasm-opt` came from the repo's pinned `binaryen` 130.0.0
devDependency and needed no provisioning.

Because the strict gate was satisfied — 18/18 on P1, the protected set
unedited, and the abandon rule untouched — Phase 2 ran. The disk gate was
re-verified immediately beforehand at 51 GiB free, above the 30 GiB minimum.
`cargo check --target wasm32-unknown-unknown -p zfb-md-wasm` compiled under
the alias, and the md-wasm suite passed **219/219 tests across 12 files**
against the 0.0.43 build.

All sixteen measured manifest fields were captured on both sides of the bump
using the same rustc 1.96.0, because the committed manifest was produced by CI
on a different toolchain and cannot be compared directly. The `0.0.31` column
is a control build of the currently committed pin; the delta column is
therefore the **bump-attributable** figure. Both columns were produced through
the documented `--update-manifest` evidence path from a timed clean-target
build, and the committed manifest was restored byte-for-byte afterwards
(sha256 `9f8fa1b8be603e655ea7a0d8830e889a712e3aea615dc8b3db5896b94bd6974b`,
verified equal to the value captured before any edit).

| Artifact | Field | `0.0.31` control | `0.0.43` | Delta | Committed (CI, v2.15.0) |
| --- | --- | --- | --- | --- | --- |
| root | `finalWasm` | 3,391,945 | 3,398,020 | +6,075 | 3,394,144 |
| root | `gzip9` | 1,520,733 | 1,523,004 | +2,271 | 1,514,540 |
| root | `glue` | 14,998 | 14,998 | 0 | 14,998 |
| root | `glueGzip9` | 4,199 | 4,199 | 0 | 4,199 |
| highlight | `finalWasm` | 1,533,152 | 1,533,104 | −48 | 1,539,186 |
| highlight | `gzip9` | 822,486 | 822,449 | −37 | 817,922 |
| highlight | `glue` | 8,758 | 8,758 | 0 | 8,758 |
| highlight | `glueGzip9` | 2,637 | 2,637 | 0 | 2,637 |
| render | `finalWasm` | 2,187,642 | 2,194,486 | +6,844 | 2,189,671 |
| render | `gzip9` | 1,093,241 | 1,095,894 | +2,653 | 1,088,858 |
| render | `glue` | 8,772 | 8,772 | 0 | 8,772 |
| render | `glueGzip9` | 2,661 | 2,661 | 0 | 2,661 |
| parse | `finalWasm` | 695,642 | 702,289 | +6,647 | 693,479 |
| parse | `gzip9` | 281,325 | 283,359 | +2,034 | 281,394 |
| parse | `glue` | 11,159 | 11,159 | 0 | 11,159 |
| parse | `glueGzip9` | 3,797 | 3,797 | 0 | 3,797 |

Four things follow from that table. First, the `0.0.31` control reproduces the
#2873 `0.0.31` column **byte-for-byte on all eight wasm figures**, which
independently corroborates a third round of the measurement method on a third
occasion. Second, **the bump moves measured wasm bytes, and by an order of
magnitude more than the 0.0.31 step did**: eleven releases of accumulated
`cst` and core work compile under the `std` feature the alias enables even
though zfb never calls them, so three of the four artifacts grow by roughly
6.1–6.8 KB raw and 2.0–2.7 KB gzipped, while `highlight-only` — which links
the least of that surface — shrinks marginally. The migration topic must
refresh the manifest; assuming a no-op is not available. Third, and unlike
#2873, **the bump is no longer small next to the CI-versus-local toolchain
gap**. On the committed pin's own artifacts that gap runs +2,199, +6,034,
+2,029 and −2,163 B, so the two effects are now comparable in size and cannot
be disentangled locally; the manager must still align the manifest from CI's
own numbers, exactly as the `a6509185` precedent in #2854 did, but this time
the alignment is correcting a real growth rather than mostly toolchain noise.
Fourth, the complete dist grew from 8,067,936 B to 8,087,454 B, +19,518 B.

Every artifact still stays under its ceiling, but **the ceiling #2851 first
flagged as tight is now genuinely tight**: `render-only` gzip-9 clears its
1,100,000 B ceiling by only **4,106 B** at 0.0.43, down from 6,759 B on the
control — the 2,653 B the bump added is 39 % of the remaining headroom.
Headroom on the other three is root 76,996 B, highlight-only 57,551 B, and
parse-only 41,641 B. Those are local figures; the headroom that actually gates
CI is computed from the committed manifest and is wider on all four — root
85,460 B, highlight-only 62,078 B, render-only 11,142 B, parse-only 43,606 B —
so projecting this round's local `render-only` delta onto CI leaves roughly
8.5 KB of headroom there. That is still a pass, and it is not a blocker for
this verdict, but at the observed growth rate `render-only` is the artifact
that will reach its ceiling first, and the pin-bump topic should say so
explicitly rather than discover it on a later release.

The lock delta under P1 was exactly the predicted minimum: `noyalib` and
`noyalib-serde-yaml` each moved `0.0.31` → `0.0.43` with the two authorized
checksums replacing the previous pair, both dependency lists unchanged, and
**597 packages before and after**; no other line of `Cargo.lock` moved. The
expanded optional-dependency set therefore cost nothing, as predicted.
`cargo tree -e normal -i noyalib` reports a single reverse path — `noyalib` ←
`noyalib-serde-yaml` ← `zfb-content` ← the rest of the workspace — identical
in shape to 0.0.31 once the version strings are normalized. `cargo deny list`
differs in exactly four strings, the two crates' versions in each of the
`Apache-2.0` and `MIT` categories, with every license category count identical
(`Apache-2.0` 288, `MIT` 430), which is what a dual-licensed in-place version
bump must look like. `cargo deny check` reported `advisories ok, bans ok,
licenses ok, sources ok` with exit code 0 both before and after, with the same
13 pre-existing wildcard warnings about internal `path` dependencies and 24
pre-existing duplicate-version warnings, and **no new exception was required**:
`deny.toml` contains no `noyalib` entry.

With row (a) confirmed at the final re-check, 18/18 on P1, the protected set
unedited, and Phase 2 green, the terminal verdict is **MIGRATE**. The pin bump
itself is a separate, separately reviewable topic; this section records only
the evidence for it, and performs no baseline refresh and no detector edit.

The other-candidate status scan found two commit-level movements and no
trigger: `serde_yaml_ng` remains 0.10.0 with canonical commit
[`36281029`](https://github.com/acatton/serde-yaml-ng/commit/3628102977f3ec9e02b95ef32fcec30b3df91390)
dated 2025-09-14; `serde_yml` remains 0.0.13 at
[`5caeeec0`](https://github.com/sebastienrousseau/serde_yml/commit/5caeeec0512296f985135502d36cd08e8ffb23d1)
dated 2026-05-28; `serde_norway` remains 0.9.42 at
[`1d37c159`](https://github.com/cafkafk/serde-norway/commit/1d37c159fc01c269a17ab72d021b271faf29472a)
dated 2024-12-21; `saphyr` remains 0.0.12 on crates.io but its `master` has
advanced to
[`92eeb0ba`](https://github.com/saphyr-rs/saphyr/commit/92eeb0babcba21ead43998ad293a8a10ccf8033f)
dated 2026-09-11; and `serde-saphyr` remains 1.2.0 on crates.io but its
`master` has advanced to
[`de014b0b`](https://github.com/bourumir-wyngs/serde-saphyr/commit/de014b0b0b677128947d73edd2f24c2036e19712)
dated 2026-09-13. Branch movement without a published version is ordinary
churn and is explicitly not a trigger kind, so none of the five is a trigger.
`noyalib-serde-yaml` is the sixth tracked entry and moved to 0.0.43, which is
the lockstep alias evaluated above rather than a separate candidate. No build
was performed for any of the six.

Two isolated target directories were used for the two timed Phase 2 builds
(`/tmp/eval0043-t43` and `/tmp/eval0043-t31`, each required by
`run-zfb-md-wasm-build-timed.mjs` to be initially absent), and Phase 1 reused
the main checkout's warm target directory; every cargo and wasm command was
serialized and this topic was the only cargo consumer on the machine. The
filesystem had 60 GiB free at the start, so the 30 GiB gate already held and
**no pre-flight deletion was performed**; it was re-verified at 51 GiB
immediately before Phase 2 and reached a 49 GiB low point during it. Each
isolated wasm target peaked at **565 MiB** — far below the 7.1 GiB #2873
measured for its full-workspace target, because these builds compile only the
`zfb-md-wasm` closure for `wasm32-unknown-unknown` — and the separate
`wasm-bindgen` CLI build used a further 375 MiB; all three were removed at the
end. The experimental manifests, lockfile, matrix printer, immutable corpus
and baseline, protected assertions, and shipped-size manifest were restored
byte-for-byte: an explicit sha256 compare over the 13 manifests, fixtures,
production sources, protected tests and the shipped-size manifest reported
zero mismatches against the values captured before any edit, and
`git status --porcelain` was empty apart from this file. On the restored tree
`cargo test -p zfb-content --test yaml_differential_harness` passed 4/4.

This section closes nothing. **No upstream report is warranted**: 0.0.43
preserves every pin zfb depends on and introduces no divergence to report.
The `render-only` headroom observation is a zfb-side budget matter, not an
upstream defect. Acknowledging the release upstream would be an outward-facing
action and remains the owner's call; it was not performed.

##### `noyalib 0.0.51` / `noyalib-serde-yaml 0.0.51` released differential evaluation (#3105) — KEEP

**Terminal verdict, recorded after the final release-race check at
2026-09-24T12:50:10Z: KEEP the production 0.0.44 pair.** Phase 1 passes,
but the candidate render-only gzip-9 artifact is 1,107,017 B against the
unchanged 1,100,000 B ceiling. A same-toolchain 0.0.44 control is also over
that ceiling (1,106,527 B); 0.0.51 adds 490 B. This is not a YAML semantic
regression, and the control proves the host already misses the budget, but
this epic requires a green Phase 2 gate before MIGRATE. Projecting a local
delta onto older CI numbers is not that evidence. No migration is authorized.

This evaluation starts from zfb commit
`1ea185c61c3bcdff79ff295a1e797745b82d4e34`, after the independent SSR identity
repair. The production pair is `0.0.44`; the candidate selected at
`2026-09-24T12:27:24Z` is the newest complete, released, non-yanked pair,
**0.0.51**. [#3045](https://github.com/Takazudo/zudo-front-builder/issues/3045)
and the detailed [#2988](https://github.com/Takazudo/zudo-front-builder/issues/2988)
record supplied the procedure, not results for this candidate.

**Independent trigger corroboration.** The evaluator downloaded and read
[workflow 35960495279](https://github.com/Takazudo/zudo-front-builder/actions/runs/35960495279)
with `gh run view 35960495279 --repo Takazudo/zudo-front-builder --log`.
Its `2026-09-24T05:33:54Z` report is `CANDIDATE_DRIFT`, exit 10, with
`errors: []`. An authenticated local `GITHUB_TOKEN=$(gh auth token) node
scripts/check-yaml-candidate-drift.mjs` at `2026-09-24T12:27:05.277Z` also
returned 10. All **39 adopted-pair triage deltas** were inspected: `noyalib`
adds versions, tags, and Releases for 0.0.46–0.0.51 (18 deltas), and
`noyalib-serde-yaml` adds those three surfaces for 0.0.45–0.0.51 (21).
The earlier bare core 0.0.45 was already observed in the committed baseline;
its alias has since shipped, completing that formerly incomplete pair.
There is no adopted yank, unyank, archive, or release-PR-state delta.

Informational changes were also read: core deletes `feat/examples-autodiscovery`,
adds `feat/v0.0.52`, and advances `main`; the alias deletes the old dependabot
branch, adds `feat/v0.0.46` through `feat/v0.0.52`, and advances `main` and
`release/v0.0.45`. Fallback `saphyr` adds 0.1.0 and its tag, advances `master`,
and deletes its renovate branch; `serde-saphyr` adds `bw/from_str_multiple`
and advances `master`. The other three fallback crates show no drift. The
role-aware policy makes those fallback and branch changes informational.
No snapshot or detector classification was changed.

The GitHub Releases for each skipped intermediate version were read. Core
0.0.45 contains CST/reader and parser-toggle fixes; 0.0.46 contains parser
contract, include-limit, parallel-work and atomic-CST changes; 0.0.47 is
release/CI maintenance; 0.0.48 adds strict query paths; 0.0.49 adds caller-owned
Rayon pools and fixes a duplicate-key fuzz oracle; 0.0.50 adds backpressured
async streams; 0.0.51 adds named parser profiles. The alias Release notes describe
the matching core-version updates. The newest-pair policy tests their accumulated
production behavior at 0.0.51, rather than shipping any intervening pin.
The passed-over pairs are recorded individually below; **both crates in
every row are non-yanked**:

| Skipped pair | Core published (UTC) | Alias published (UTC) | Reason |
| --- | --- | --- | --- |
| 0.0.45 | 2026-09-18T18:05:28.349263Z | 2026-09-19T07:41:27.464727Z | Superseded by complete 0.0.51 |
| 0.0.46 | 2026-09-21T15:04:50.854459Z | 2026-09-21T22:31:50.381115Z | Superseded by complete 0.0.51 |
| 0.0.47 | 2026-09-21T22:54:39.552783Z | 2026-09-21T23:10:08.420790Z | Superseded by complete 0.0.51 |
| 0.0.48 | 2026-09-22T00:59:43.932787Z | 2026-09-22T07:23:53.360055Z | Superseded by complete 0.0.51 |
| 0.0.49 | 2026-09-22T09:31:12.915663Z | 2026-09-22T09:49:53.603529Z | Superseded by complete 0.0.51 |
| 0.0.50 | 2026-09-22T12:02:45.101054Z | 2026-09-22T12:21:53.235495Z | Superseded by complete 0.0.51 |

**Candidate provenance.**

| Field | `noyalib 0.0.51` | `noyalib-serde-yaml 0.0.51` |
| --- | --- | --- |
| Published (UTC) | `2026-09-22T14:47:58.355202Z` | `2026-09-22T15:04:44.656167Z` |
| Archive SHA-256 | `77d00fc12936a62c370ddaaba83218c753ba110b34a60ed6e278fd7b7389e350` | `dac0eb0d33c4def47b63dcbad6024ce2b5b2cb212af8de155224585a126081bb` |
| Archive bytes | `1214177` | `69286` |
| License | `MIT OR Apache-2.0` | `MIT OR Apache-2.0` |
| Minimum Rust | `1.86.0` | `1.86.0` |

The corresponding [core registry record](https://crates.io/api/v1/crates/noyalib/0.0.51)
and [alias registry record](https://crates.io/api/v1/crates/noyalib-serde-yaml/0.0.51)
were checked against the actual downloaded `.crate` archives and, **before
any candidate compilation**, the checksums written by Cargo. All agree.
The core annotated tag object `8235291f76bc63a3a5407f57d69540af442a6a05`
(tagged `2026-09-22T18:50:07Z`) dereferences to
[`c2646cdc`](https://github.com/sebastienrousseau/noyalib/commit/c2646cdc88ae816711f06646020201b50e4cb99b);
its [Release](https://github.com/sebastienrousseau/noyalib/releases/tag/v0.0.51)
was published `2026-09-22T14:46:54Z`. The alias tag object
`48676fd3d90e0857031b38d1874c43254dbbbec1` (tagged `2026-09-22T18:50:23Z`)
dereferences to
[`3cd031df`](https://github.com/sebastienrousseau/noyalib-serde-yaml/commit/3cd031df4365b14fdb4f92aee317495e7de394fc);
its [Release](https://github.com/sebastienrousseau/noyalib-serde-yaml/releases/tag/v0.0.51)
was published `2026-09-22T18:53:06Z`. Both Release bodies publish the matching
archive checksum. These are the observed API dates, including the core tag's
later timestamp than its Release; no inferred chronology substitutes for them.

The published alias's manifest and source were inspected: `build = false`, no build script,
`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`, and the sole implementation
`pub use noyalib::compat::serde_yaml::*;`. Its only normal dependency is exact
`noyalib =0.0.51`, `default-features = false`, features `std` and
`compat-serde-yaml`. The published core moved its test into
`tests/integrations/serde_yaml_contract.rs`; that source still predicts the
custom-tag error at `(1, 8, 7)` with Display column 8. No upstream test,
example, benchmark, or unreleased branch was executed.

**Open release work is a separate future trigger.** Independent API inspection found
[core PR #459](https://github.com/sebastienrousseau/noyalib/pull/459) and
[alias PR #28](https://github.com/sebastienrousseau/noyalib-serde-yaml/pull/28),
both open on `feat/v0.0.52`. They are explicitly dispositioned as unreleased
future work, not included in the evaluated 0.0.51 package. Both watcher
`pendingReleasePr` entries are currently `null`, so discovering those PRs was
an independent triage observation, not an automated release-PR-state delta.
The final confirmation topic must revisit them and any completed 0.0.52 pair
before deciding what its late snapshot may acknowledge.

**Phase 1, P1 only.** The temporary root manifest changed the package alias
pin from `=0.0.44` to `=0.0.51`. There were **zero production source lines**,
zero corrective rounds, and no unsafe, FFI, OS-specific branches, polling,
signal supervision, or Unicode tables. The alias and the direct compatibility
module are the same implementation, and P1 matched all 18 cases, so the
conditional P3 direct-shim experiment was not needed. Neither the immutable
corpus nor baseline nor harness was edited; `CURRENT_ADAPTER_NAME` and the
fixture's adapter metadata deliberately continued to agree at `0.0.44`.
KEEP leaves both strings unchanged; only a separately authorized migration
may relabel them together. In this matrix,
`match` means complete JSON equality or exact error Display plus optional
line, column, and byte-index equality:

| Category | Corpus case | P1 package alias |
| --- | --- | --- |
| anchors-aliases | `anchors-and-aliases` | match |
| merge-keys | `merge-key-is-an-ordinary-json-key` | match |
| non-string-keys | `non-string-scalar-keys` | match |
| non-string-keys | `non-string-composite-key` | match |
| scalar-edge-cases | `yaml-11-boolean-spellings` | match |
| scalar-edge-cases | `octals-sexagesimals-and-numbers` | match |
| scalar-edge-cases | `null-and-date-scalars` | match |
| unicode-bom-crlf-emoji | `unicode-crlf-and-emoji` | match |
| malformed-input | `malformed-unicode-location` | match |
| malformed-input | `malformed-flow-sequence-at-eof` | match |
| malformed-input | `malformed-indentation` | match |
| explicit-tags | `built-in-explicit-tags` | match |
| explicit-tags | `custom-explicit-tag` | match |
| duplicate-keys | `duplicate-map-keys-last-wins` | match |
| non-finite-overflowing-numbers | `non-finite-and-overflowing-numbers` | match |
| non-finite-overflowing-numbers | `integer-boundaries` | match |
| non-finite-overflowing-numbers | `integer-overflow` | match |
| alias-anchor-resource-limits | `alias-anchor-repetition-limit` | match |

`cargo test -p zfb-content --test yaml_differential_harness` passed 4/4,
including the single assertion comparing all 18 observations. The complete
`cargo test -p zfb-content` then passed **1,072/1,072** across 34 test/doc-test
groups, including `error_messages.rs` 2/2, with zero ignored tests.
`cargo test -p zfb-md-wasm --test api --test parse_to_ast` passed **57/57**
(32 + 25), including the protected EOF frontmatter diagnostic (original
source line 3, column 1) and original-source UTF-16 column 9. All three
separate native checks passed: `cargo check -p zfb-content`, `cargo check -p
zfb-md-wasm`, and `cargo check -p zfb --no-default-features`. Keeping them
separate avoids feature unification enabling V8 in the CLI check. The last
command emits the same 28 feature-gated warnings already recorded in #2988.

`cargo test -p zfb --no-default-features --lib diagnostics::tests` passed
**11/11**, including `frontmatter_yaml_error_locates_within_user_file`, with
zero ignored tests. An initial invocation with `--bin zfb` selected zero
tests; that invocation is explicitly **not coverage**. The corrected library
selection above provides the evidence. No expectation or source change was
needed. Phase 1 therefore permits Phase 2.

**Phase 2 toolchain and commands.** This round uses Linux x86_64 (WSL),
`rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0 (f2d3ce0bd
2026-03-21)`, `wasm-bindgen 0.2.121`, `wasm-opt version 130
(version_130)`, Node `v24.13.1`, pnpm `11.3.0`, and `cargo-deny 0.19.9`.
The wasm32 standard library and exact bindgen CLI were already installed.
The rustup stable toolchain is used throughout; this evaluation provides no
Mac, Windows, or different-compiler observation. Native commands reuse
`/home/takazudo/.cargo-target` from the existing Cargo configuration; no
shared target was cleaned. `pnpm install --frozen-lockfile` supplied the CLI
build-script prerequisites and changed no tracked files.

Every Rust build/test ran through
`bash "$HOME/.codex/scripts/heavy-guard.sh" -- <command>`; the protected
native checks were sequential inside one guarded shell script. The manager
ran the two long artifact builds through the same guard. All Phase 1 and
wasm32-check guards reported PASS. The candidate artifact build reported
**FAIL**, exit 1, in 118 seconds, with a real size assertion; it was not
ENV_SUSPECT or contention. The follow-up script completed and its outer guard
reported PASS, but its individual control-build status was **exit 1**, which
is the status used here. No deferred check counts as a pass. The disk gate
was rechecked immediately before Phase 2: **451 GiB free**, above 30 GiB.

`cargo check -p zfb-md-wasm --target wasm32-unknown-unknown` passed. These
were the exact artifact and package-test commands, in execution order:

```sh
CARGO_TARGET_DIR=/tmp/zfb-3105-wasm-candidate node scripts/run-zfb-md-wasm-build-timed.mjs
pnpm --filter @takazudo/zfb-md-wasm test
# Restore the original 0.0.44 manifest/lock for the control measurement.
CARGO_TARGET_DIR=/tmp/zfb-3105-wasm-control node scripts/run-zfb-md-wasm-build-timed.mjs
cargo deny list
cargo deny check
```

Both timed target directories were initially absent. Both builds emitted all
four artifacts, ran TypeScript compilation, printed the sixteen-field summary,
and then failed at the existing render-only gzip ceiling. The candidate's
emitted artifacts passed **219/219 tests in 12 files** under the direct
package-test command above. The composite `pnpm test:md-wasm` was **not run**:
its same build step had already failed; running its test half independently
does not make the composite gate green. Likewise, the planned
`node scripts/assert-zfb-md-wasm-budgets.mjs --build-log <log> --dist
crates/zfb-md-wasm/npm/dist --update-manifest` steps were **not reached**
after the build failure. The table records the actual production build
summaries at each pin, not a successful manifest refresh. The committed size
manifest and every ceiling remain byte-identical to the starting revision.

| Artifact | Field | `0.0.44` control | `0.0.51` candidate | Delta |
| --- | --- | --- | --- | --- |
| default | `finalWasm` | 3,421,204 | 3,422,238 | +1,034 |
| default | `gzip9` | 1,541,847 | 1,542,918 | +1,071 |
| default | `glue` | 14,998 | 14,998 | +0 |
| default | `glueGzip9` | 4,199 | 4,199 | +0 |
| highlight-only | `finalWasm` | 1,533,609 | 1,533,609 | +0 |
| highlight-only | `gzip9` | 822,251 | 822,265 | +14 |
| highlight-only | `glue` | 8,758 | 8,758 | +0 |
| highlight-only | `glueGzip9` | 2,637 | 2,637 | +0 |
| render-only | `finalWasm` | 2,215,286 | 2,216,153 | +867 |
| render-only | `gzip9` | 1,106,527 | 1,107,017 | +490 |
| render-only | `glue` | 8,772 | 8,772 | +0 |
| render-only | `glueGzip9` | 2,661 | 2,661 | +0 |
| parse-only | `finalWasm` | 723,818 | 724,614 | +796 |
| parse-only | `gzip9` | 296,318 | 296,583 | +265 |
| parse-only | `glue` | 11,159 | 11,159 | +0 |
| parse-only | `glueGzip9` | 3,797 | 3,797 | +0 |

| Artifact | Gzip-9 ceiling | Candidate headroom | Result |
| --- | --- | --- | --- |
| default | 1,600,000 | 57,082 | PASS |
| highlight-only | 880,000 | 57,735 | PASS |
| render-only | 1,100,000 | -7,017 | FAIL |
| parse-only | 325,000 | 28,417 | PASS |

The unchanged root/render/parse glue columns corroborate that this is a
parser-code size comparison. The highlight wasm byte count is unchanged
while gzip differs by 14 B; same-length binaries need not have identical
bytes, and no byte-identity claim is made. The control proves that the local
budget breach predates the candidate. The candidate increases render-only
by 490 B gzip, so it does not repair that failed gate. No ceiling exception,
`--allow-over-ceiling`, updated expectation, or CI projection was used.

**Dependency and license audit.** `cargo tree -e normal -i noyalib` shows the
single edge through `noyalib-serde-yaml` and `zfb-content`; `cargo tree -e
features -i noyalib` confirms the alias's `std`/`compat-serde-yaml` feature
closure. The lockfile has **597 packages before and after**, and the exact
delta is the two adopted entries' version/checksum fields only. Both
normal-dependency arrays and every other entry are unchanged. No optional
package, transitive package, or dependency exception entered the graph.
`cargo deny list` at both pins is byte-identical after normalizing only the
two YAML version strings; all sixteen license-category counts are unchanged
(including Apache-2.0 288 and MIT 430). `cargo deny check` passes at both
pins: `advisories ok, bans ok, licenses ok, sources ok`. Existing wildcard
and duplicate-version warnings require no new exception.

**Final release-race guard.** Immediately before recording KEEP, the registry,
actual archive SHA-256 values, exact alias dependency, selected tag objects
and commits, GitHub Releases/checksums, and open release PRs were rechecked
from `2026-09-24T12:50:03Z` through `2026-09-24T12:50:10Z`. Both crates still
top out at 0.0.51, remain non-yanked, retain the two checksums above, and the
alias still pins `noyalib =0.0.51` with the same features. Tags/commits match
the initial observation; the two 0.0.52 PRs remain open and unreleased.
A final authenticated `node scripts/check-yaml-candidate-drift.mjs --json`
at `2026-09-24T12:50:03.495Z` returned **10**, `errors: []`, with the same
39 triage deltas and informational changes. There is no newly published pair
to add to this evaluation; a later complete release remains a new trigger.

**Downstream contract and next trigger.** Issue
[#3106](https://github.com/Takazudo/zudo-front-builder/issues/3106) is locked
to KEEP: preserve `serde_yaml = { package = "noyalib-serde-yaml", version =
"=0.0.44" }`, its two existing lock entries, `CURRENT_ADAPTER_NAME`, and the
baseline's adapter metadata. No candidate pin, production source, fixture,
size manifest, or ceiling update is authorized. All 18 observations, the
corpus, error Display/byte coordinates, original-source UTF-16 diagnostic
pins, and the SSR identity repair remain protected. Applying KEEP records the
reason and next trigger without repeating this rejected migration. The late
confirmation topic alone may refresh the watcher snapshot after checking
this disposition and any new deltas. This topic never refreshes it.

A completed non-yanked pair beyond 0.0.51, a yank of the adopted 0.0.44 pair,
or fresh green budget evidence for 0.0.51 can open a new bounded evaluation.
Open 0.0.52 PRs are explicitly pending; neither their branch existence nor
this KEEP permits a future completed release to be silently acknowledged.
The failed local budget remains a limitation, not deferred passing evidence.
No upstream bug report is warranted from these results: all semantic and
location contracts pass, and the size breach also exists on the control.

**Restoration and final verification.** The temporary root manifest and lockfile
were restored, and SHA-256 checks over 15 protected files found zero changes:
the two manifests, lockfile, harness, corpus, immutable baseline, protected
content/md-wasm tests and production sources, CLI diagnostics, shipped-size
manifest, watcher baseline, and the preceding SSR repair's bundler source.
The restored 0.0.44 harness passed **4/4** again (all 18 observations match),
with no skipped tests. `pnpm format:check` and `git diff --check` pass. The
two isolated wasm targets were removed; shared native build caches were kept.
The only committed file is `DEPENDENCIES.md`; there are no package/source
changes that require committed generated artifacts. No browser or visual
check applies to this evidence-only change. No upstream message was sent.

##### Epic #3103 wave 4 final confirmation (#3107) — KEEP, one snapshot refresh

The installed YAML pair remains **0.0.44**, as required by #3106. `Cargo.toml`
still pins `noyalib-serde-yaml =0.0.44`; both lock entries and the harness's
`CURRENT_ADAPTER_NAME` still identify 0.0.44. No production manifest, lockfile,
corpus, harness, or adapter metadata changed.

**Late adopted-pair classification.** The authenticated detector report at
`2026-09-24T13:04:30.295Z` returned `CANDIDATE_DRIFT`, report `exitCode: 10`,
and `errors: []`. Its JSON contains exactly the already-evaluated 39 triage
deltas from #3105: `noyalib` publishes, tags, and releases 0.0.46–0.0.51
(18 deltas); `noyalib-serde-yaml` publishes, tags, and releases 0.0.45–0.0.51
(21 deltas). The first shell wrapper itself returned 1 because zsh reserves
`status`; the JSON report was written before that wrapper error. A corrected
authenticated invocation at `2026-09-24T13:08:06.809Z` captured the detector's
actual exit 10 and the same `CANDIDATE_DRIFT` report with `errors: []`.
Neither observation contains a publish, tag, Release, yank/unyank, archive, or
unarchive delta beyond the versions already evaluated at
`2026-09-24T12:50:03.495Z` in #3105. Both crates still top out at 0.0.51 and
have no yanked versions. No new release or yank needed evaluation before the
snapshot.

The other adopted-pair changes are informational branch movement already
recorded by #3105: `noyalib` deleted `feat/examples-autodiscovery`, added
`feat/v0.0.52`, and advanced `main`; the alias deleted its old dependabot
branch, added `feat/v0.0.46` through `feat/v0.0.52`, and advanced `main` and
`release/v0.0.45`. The fallback candidate changes remain informational under
the role-aware policy, including `saphyr 0.1.0` and ordinary branch movement.

**Evidence-backed release-PR tracking.** Authenticated GitHub reads confirmed
core PR [#459](https://github.com/sebastienrousseau/noyalib/pull/459) and alias
PR [#28](https://github.com/sebastienrousseau/noyalib-serde-yaml/pull/28) are
both still OPEN on `feat/v0.0.52`. PR #459 advances core release and installation
surfaces to 0.0.52; PR #28 prepares the alias's exact core pin for 0.0.52 and
states its temporary Git override is removed after publication. Neither crate
has published 0.0.52, so these are pending future work, not evaluated packages.
The watcher now tracks these specific, evidenced PRs as
`noyalib.pendingReleasePr = 459` and
`noyalib-serde-yaml.pendingReleasePr = 28`. The snapshot records both as OPEN;
future state changes are visible to the detector. This pointer update is the
recorded disposition of the previously human-observed PRs, not a completed
release or a change to the KEEP verdict.

**Single late refresh and live confirmation.** Following the documented
write-to-temp-then-copy recipe, the one generated snapshot has
`checkedAt = 2026-09-24T13:11:22.016Z`, `schemaVersion: 3`, and the original
anti-gaming comment. It includes the evaluated 0.0.46–0.0.51 release set, the
informational candidate/branch observations, and the two OPEN PR pointers.
Prettier formatted the snapshot. The post-refresh authenticated detector at
`2026-09-24T13:13:08.512Z` returned exit 0, `no-drift`, and `errors: []` for all
seven candidates. No further refresh was made.

**Confirmation checks.** The detector unit suite passed **77/77**; Prettier
check and `git diff --check` passed for the detector, unit test, and snapshot.
The guarded distinct-physical-version unit test passed 1/1. The generic
nested-workspace SSR identity build passed 1/1. The public
`public_zudo_sg_catalog_renders_external_hook_story` fixture was run four
times under `heavy-guard`. The initial uninstrumented run failed 1/1 before
rendering: plugin-host `init` exceeded its 120-second deadline (test duration
144.80 seconds), and the guard recorded `FAIL exit=101`, with minimum free
memory of 4,279 MiB. This is a real first-run failure and remains in the
record.

Two diagnostic reruns passed 1/1 under the guard, with minimum free memory of
8,581 MiB and 8,112 MiB. The first used `NODE_DEBUG=esm`; the second wrapped
Node externally to capture the plugin-host exchange while preserving the
fixture and product behavior. The captured init request contained all eight
resolved module URLs actually sent by `PluginSpec.module`, covering the five
zudo-doc and three zudo-sg plugins; the host replied `loaded: 8`, then completed
virtual loads, `preBuild`, `postBuild`, and shutdown. The 17.6 MB ESM stderr
capture contained no module-import or plugin-host error. These runs verify the
exact resolved imports and successful host reply for that attempt, but they do
not reproduce or explain the initial timeout.

The final uninstrumented/default fixture run passed 1/1 under the guard
(8.60 seconds test time; 54 seconds total; minimum free memory 7,954 MiB).
Across the four public runs, the single timeout coincided with a lower recorded
minimum-free-memory value than all three passing attempts. This correlation is
not proof that memory caused the timeout, and a later pass does not erase the
initial FAIL. No timeout or test expectation was changed; classify the public
fixture result as an intermittent plugin-host-init risk until the receiving
manager compares it against the merged-base health run.

The guarded package-route controls passed 6/6:
`user_page_bare_import_is_staged_from_package_route_overlay`,
`workspace_package_routes_and_virtual_host_hooks_share_staged_preact_identity`,
`user_page_relative_import_resolves_with_package_route_present`,
`nested_package_route_imports_correct_module`,
`package_route_with_relative_import_bundles`, and
`no_package_routes_build_is_unaffected`. With the pinned esbuild explicitly
supplied, the two ignored stage-escape controls passed 2/2:
`real_esbuild_now_rejects_root_workspace_child_package_escape_through_node_modules_symlink`
and
`empty_exclude_workspace_package_exact_staging_does_not_disarm_the_audit_at_root_claimed_workspace`.
An earlier unqualified `--exact` invocation selected zero unit tests, and the
first stage-escape invocations were ignored without `--ignored`; none of those
zero-test invocations counts as coverage. No browser or visual check applies.

The receiving manager still owns `pnpm b4push` on the merged base and the T1
required GitHub gate: `health`, `build (no-v8)`, both Linux binary builds, the
four local-mode smoke jobs, packed-tarball Scaffold E2E, `pnpm audit (prod)`,
and `Docs gate`. In particular, compare the real public `withZudoSg` result
against the merged-base health run before treating the timeout as resolved.

**Plugin init follow-up (#3120, 2026-09-25).** The exact public fixture passed
in [PR #3111 health run 36018785652](https://github.com/Takazudo/zudo-front-builder/actions/runs/36018785652)
at head `7cef707728a9a1d889cb1422f2ce6eed77608419` (10.234s), in the
[merged-base run 36021760259](https://github.com/Takazudo/zudo-front-builder/actions/runs/36021760259)
at `06f8b23372a3faf089c8ff90a3413cad3c374511` (10.467s), and in the
[later main run 36027839583](https://github.com/Takazudo/zudo-front-builder/actions/runs/36027839583)
at `89463b33362647c0d922dcdef61b280ac5ad8cfd` (10.194s). These exact
health-log outcomes, plus the later
[super-base run 36044799357](https://github.com/Takazudo/zudo-front-builder/actions/runs/36044799357)
at `39d6ba7b9f1bb3886aadc1d66e7829a807f09f1a` (9.879s), establish
recurrence was not observed on those runs;
they do not supply a trace for the initial local 120s failure or identify its
cause. The 144.80s failure and 4,279 MiB minimum free memory above remain
part of the evidence, with memory causation unproven.

`ZFB_PLUGIN_INIT_TRACE=1` now adds bounded stderr phase lines with a per-host
and request ID plus elapsed milliseconds. The parent records request write
start/completion, reply read, timeout, and child reaping; the child records
receipt, each numbered module import start/end, and reply write completion.
The real public fixture enables it only for its subprocess, so a future
captured assertion failure can distinguish a write delay, an import stall,
and a missing reply without printing module URLs, options, or source. The
120s deadline and fixture assertions are unchanged. A pass with tracing is
evidence only for that attempt; the intermittent init risk remains open.

**Plugin init decision (#3121, 2026-09-25): OBSERVE.** Retain #3120's
diagnostics and investigate the next recurrence through the dedicated open
tracker [#3127](https://github.com/Takazudo/zudo-front-builder/issues/3127).
No available evidence demonstrates the cause of the original timeout, so this
decision delivers observability and makes no claim that the timeout is fixed.
The manager updated
[#3122](https://github.com/Takazudo/zudo-front-builder/issues/3122)'s confirmation
contract and created #3127 before this decision task began.

The first guarded public fixture on diagnostic merge
`76c84ac6f4a74273201b41f8f4c1d5a62562f653` **failed** after 11.57s
(`FAIL exit=101`; 154s including build; minimum free memory 10,843 MiB).
Its captured tool output shows parent write/flush completion at 6ms, all eight
child imports ending by 2,095ms on the child's clock, and parent `reply_read`
at 2,494ms on the parent's clock. Initialization succeeded in this attempt;
the later failure was CSS Modules virtual-module preprocessing rejecting
`/var/folders/.../stories/hooked.stories.tsx` as outside a
`/private/var/folders/...` project root. The identical guarded command on
pre-epic SHA `39d6ba7b9f1bb3886aadc1d66e7829a807f09f1a`, after
`pnpm install --frozen-lockfile`, failed with the same signature after 13.12s
(`FAIL exit=101`; 204s including build; minimum free memory 10,642 MiB).
The initial baseline attempt without installed dependencies failed during
build setup and supplies no fixture result. Separate issue
[#3126](https://github.com/Takazudo/zudo-front-builder/issues/3126) owns the
pre-existing macOS path failure. These local runs are FAIL, not successful
catalog-rendering checks. The four exact CI passes above establish only
non-recurrence on their recorded SHAs; they do not verify this decision's
eventual merged SHA.

The alternatives remain evidence-dependent:

| Option                                                      | Decision and evidence needed                                                                                                                                                                                            |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Retain diagnostics and observe                              | Chosen: there is no failing init trace to attribute. #3127 keeps the unresolved risk actionable.                                                                                                                        |
| Change import or promise handling                           | A future trace must identify the stalled numbered import, followed by a deterministic failing-before regression of its actual mechanism. Fast direct imports and successful later runs do not establish that mechanism. |
| Change IPC or child lifecycle                               | Require evidence locating the lost request/reply or unexpected exit and a failing-before regression. A synthetic reply after EOF is not a child wire reply.                                                             |
| Change scheduling or memory behavior                        | Require evidence beyond the original lower-free-memory correlation. The guarded runs do not isolate scheduling or memory as a cause.                                                                                    |
| Raise the deadline, weaken the fixture, or rewrite the host | Rejected: none is justified by this evidence, and each would obscure the original contract.                                                                                                                             |

**Assumptions and remaining risks.** Tracing can change timing, and passing
runs cannot exclude an intermittent fault. Elapsed times are local to each
process's start; do not subtract child time from parent time to infer IPC
latency. Stderr delivery order can lag execution, so the terminal last phase
is the last observed phase, not proof of the precise stall location. The
existing 120s budget covers waiting for a reply **after** stdin flush; it is
not an end-to-end boot deadline and does not bound a blocked write/flush.
Missing phases may therefore require external process/runner evidence.
Diagnostics omit options, module URLs, and source; ordinary plugin stderr
and error text still require review before sharing outside the trusted logs.

**Recurrence and PR-time acceptance.** On another plugin `init` timeout or
unexpected host exit in the public fixture, retain its complete failure log,
including every `[zfb-plugin-init]` line and terminal error. Record the SHA,
OS/runner, exact command, duration, CI job URL or local heavy-guard verdict and
minimum free memory. Correlate host/request IDs, parent write/flush, child
receipt and numbered imports, reply write/read, timeout/exit and cleanup;
keep secrets and full module URLs out of the tracker. Follow #3127's capture
instructions and require a deterministic failing-before regression before
claiming a bounded causal fix. Keep #3127 open through this sweep.

For #3122, rerun the focused
`plugin_runner::tests::traced_init_reaps_stalled_child_and_keeps_success_compatible`
test on the integrated base (delayed success, unresolved import timeout/reap,
and exited child). Confirm trace opt-in remains subprocess-scoped, phase
records remain bounded and correlated, terminal timeout/exit errors retain
the last observed phase, and diagnostic fields contain no request data.
The public fixture must still assert external-hook catalog output with the
same 120s default, assertions, and YAML KEEP pins. Classify the known local
macOS signature as pre-existing FAIL under #3126; any different failure needs
new attribution. The manager owns resource-serialized heavy checks and the
required PR gates on the integrated SHA. CI must show no new failures and
the exact public fixture outcome; a pass there remains non-recurrence.

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

* **syntect 5.3.0 — RETAIN; watch 6.0.0.** Checked 2026-09-02:
  [crates.io still tops out at 5.3.0](https://crates.io/crates/syntect/versions),
  and [master HEAD `4aa78031`](https://github.com/trishume/syntect/commit/4aa78031)
  is dated 2026-04-28 with `Cargo.toml` still declaring 5.3.0. The
  [6.0 milestone](https://github.com/trishume/syntect/milestone/3) remains 14
  open / 3 closed, while [PR #700](https://github.com/trishume/syntect/pull/700)
  is still changing the parse-output contract. There is no security reason to
  move: `cargo deny check advisories` passes; the bincode 1.3 advisory is
  already explicitly tracked under #1373, and master still uses bincode 1.3.

  Adoption requires regenerating the committed packdump because its format no
  longer deserializes on master. Master's 192 default syntaxes already include
  TypeScript, TSX, and TOML, so the extra assets can then be removed. The
  pre-computed cost is at least +262 KB of gzip-incompressible dump and about
  +31 KB of wasm code against the highlight artifact's former 61,752 B of
  headroom, requiring either a new ceiling decision or a curated dump rebuilt
  from scratch. The JavaScript grammar also jumps from 2017 to 2026, requiring
  regeneration of the onig snapshots and both parity oracles. The source API
  adaptation is small: about two `.ops` lines for `ParseState::parse_line`'s
  new return type.

  Re-evaluate on any of these explicit triggers: (1) a crates.io release at or
  above `6.0.0-alpha`; (2) a `v6*` tag or master `Cargo.toml` version bump; or
  (3) merge of upstream #700 and #701, the earliest point at which a git-SHA
  spike is worthwhile. A git pin or vendoring before trigger (3) is not
  migration-eligible.
* **Platform/process utilities (#2751) — KEEP.** `local-ip-address` enumerates
  every non-loopback IPv4 interface for bind-all ready URLs; a UDP-connect
  shortcut would select only one egress address, and the repository has no
  Windows Rust test lane to prove equivalent interface enumeration. `wait-timeout`
  provides the cross-platform `ChildExt::wait_timeout` path that kills and
  reaps a timed-out child; replacing it with polling is both a semantic change
  and an abandon-rule trigger. #2826 is a human-initiated reopen with new
  upstream evidence: `@takazudo/zudo-doc` now ships the `run-parallel`
  supervisor. The contrary gate this KEEP demanded is met: it forwards signals
  to the full descendant tree, propagates the real exit code, and reaps both
  children on Ctrl-C. Zero lines of replacement code live in this repository,
  so the abandon rule's signal-supervision trigger does not apply. The lockfile
  loses 19 package records and 106 non-blank lines: the former process-supervisor
  package at `7.0.2`,
  `ansi-styles@6.2.3`, `cross-spawn@7.0.6`, `path-key@3.1.1`,
  `shebang-command@2.0.0`, `shebang-regex@3.0.0`, `which@2.0.2`,
  `isexe@2.0.0`, `memorystream@0.3.1`, `minimatch@9.0.9`,
  `brace-expansion@2.1.2`, `balanced-match@1.0.2`, `pidtree@0.6.1`,
  `read-package-json-fast@4.0.0`, `json-parse-even-better-errors@4.0.0`,
  `npm-normalize-package-bin@4.0.0`, `shell-quote@1.10.0`, `which@5.0.0`,
  and `isexe@3.1.5`. The full audit also loses its two high
  `brace-expansion` advisories (GHSA-mh99-v99m-4gvg and
  GHSA-rgw5-rvv9-x895); the gated production audit is unchanged.
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
| `docs/package.json` | Private docs site; zudo-doc stack, the intentional peer keep-list, TypeScript/types, `html-validate`, `vitest`, and Wrangler. | Clean after #2746 removed `pagefind`, `remark-directive`, and redundant `gray-matter`; #2825 removed the stale runtime-import keep-list, and zudo-doc 5.14.0's removal of the `gray-matter`/`js-yaml` chain retired the override in #2823. #2826 replaces the separate docs process supervisor with zudo-doc's `run-parallel`, which forwards signals, propagates real exit codes, and reaps both children. |
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
