# YAML 0.0.55 differential evaluation #4049 — BLOCKED

**Phase 1 compatibility passed; Phase 2 is blocked by the unchanged 30 GiB free-disk gate. No terminal KEEP/MIGRATE verdict is supported.** Both pins remain production-compatible across the existing tested surface. The production dependency remains `=0.0.44`; no baseline refresh, ceiling change, source adaptation or release is authorized by this record.

The targeted comparison used source `cb4b6dbd3af21980b302916c5d97d4f767c51574`; broader Phase 1 and recovery used `a465edb4fe8426aa9b31a897adb71d41bc9e0d0e`, which adds evaluation files only. Manager ran all Rust work serially through heavy-guard with one shared `/workspace/scratch/zfb-sweep-261008/rust-target`. Only the temporary root package-alias version and the two Cargo lock entries changed. The immutable corpus, baseline, adapter label, protected assertions and production sources remained byte-identical: **zero production source lines and zero source corrective rounds**.

## Candidate and final release race

The latest complete released non-yanked pair is **0.0.55**, superseding the older 0.0.54 alert. `provenance.json` includes each skipped 0.0.45–0.0.54 release's timestamp/yank state and the inspected published manifests; `github-provenance.json` contains initial tag objects, commits and Release bodies.

| Crate | SHA-256 | Registry published UTC | Archive bytes |
| --- | --- | --- | --- |
| noyalib | `0c3ccce0306ea4477a655985ff2bf3e6c38f78f3e1defef6f30c7eb24d58cf41` | 2026-10-08T06:37:46.465213Z | 1,274,912 |
| noyalib-serde-yaml | `4775f0afd3f1d5e55b3779e8e4b7ffde09d690629f6731e4047159a751c17a3b` | 2026-10-08T07:10:21.924189Z | 72,952 |

Actual downloaded archives, registry checksums, GitHub Release archive digests and Release-body checksums agree. Published alias: exact core `=0.0.55`, default features false, features `std` and `compat-serde-yaml`; `build = false`, unsafe forbidden, sole implementation a direct compat re-export. Both licenses `MIT OR Apache-2.0`, minimum Rust `1.86.0`. Upstream examples and tests were read, never executed.

The final registry/archive/manifest check began **2026-10-08T07:52:25.864940Z** after native checks. Both adopted 0.0.44 versions and both candidate versions remain non-yanked; neither a newer complete pair nor a newer half-pair exists. Independent connected GitHub reads **07:52:49.350–07:52:52.054Z** confirmed neither repository archived, zero open pull requests in both complete collections, and unchanged annotated tags/Releases/checksums:

| Crate | Annotated tag object | Dereferenced commit | Release published UTC |
| --- | --- | --- | --- |
| noyalib | `65c55452c7f419fbb672d67705269583745928a4` | `8adef27f177e03f07ac0e96ea9a8a774b21bee76` | 2026-10-08T06:37:35Z |
| noyalib-serde-yaml | `3cda03c23b0a2cfe27c1b7e548421494c8bd3a72` | `21a0ff6424367a0b47b5d3110a6990767395114e` | 2026-10-08T07:10:11Z |

Release-race row (a): the selected complete release remains current; no release-selection invalidation observed. This is a bounded observation, not future monitoring. Raw final records: `yaml-remaining/final-registry.log`, `yaml-final-github.json` under `/workspace/scratch/zfb-sweep-261008/`. The existing authenticated drift detector was **NOT RUN** in this final round; no detector exit or delta claim is made. Direct registry/GitHub checks above do not refresh its baseline.

## Exact unchanged 18-case comparison

Baseline harness: 4/4, guard PASS 64s. Candidate targeted harness: 4/4, guard PASS 52s; broader Phase 1 repeats candidate 4/4. All 18 observations match; equality covers complete JSON or exact Display plus optional line/column/byte index. Both hard original-source mapping pins pass. The 0.0.55 Release predicts duplicate-key refusal, but the consumed Value path still passes the unchanged last-wins assertion. Conditional P3 is **NOT APPLICABLE** because P1 is 18/18; no direct-shim experiment ran.

| Case | Category | Baseline 0.0.44 | P1 0.0.55 |
| --- | --- | --- | --- |
| `anchors-and-aliases` | anchors-aliases | match | match |
| `merge-key-is-an-ordinary-json-key` | merge-keys | match | match |
| `non-string-scalar-keys` | non-string-keys | match | match |
| `non-string-composite-key` | non-string-keys | match | match |
| `yaml-11-boolean-spellings` | scalar-edge-cases | match | match |
| `octals-sexagesimals-and-numbers` | scalar-edge-cases | match | match |
| `null-and-date-scalars` | scalar-edge-cases | match | match |
| `unicode-crlf-and-emoji` | unicode-bom-crlf-emoji | match | match |
| `malformed-unicode-location` | malformed-input | match | match |
| `malformed-flow-sequence-at-eof` | malformed-input | match | match |
| `malformed-indentation` | malformed-input | match | match |
| `built-in-explicit-tags` | explicit-tags | match | match |
| `custom-explicit-tag` | explicit-tags | match | match |
| `duplicate-map-keys-last-wins` | duplicate-keys | match | match |
| `non-finite-and-overflowing-numbers` | non-finite-overflowing-numbers | match | match |
| `integer-boundaries` | non-finite-overflowing-numbers | match | match |
| `integer-overflow` | non-finite-overflowing-numbers | match | match |
| `alias-anchor-repetition-limit` | alias-anchor-resource-limits | match | match |

## Protected Phase 1 and audit evidence

| Command / check | Observed result | Evidence under scratch directory |
| --- | --- | --- |
| `cargo test --locked -p zfb-content --test yaml_differential_harness` | PASS 4/4, including 18-case equality | `yaml-phase1/candidate-harness.log` |
| `cargo test --locked -p zfb-content` | PASS 1,081 across 34 test/doc groups, zero ignored; `error_messages` 2/2 | `yaml-phase1/candidate-content.log` |
| `cargo test --locked -p zfb-md-wasm --test api --test parse_to_ast` | PASS 58/58 (33 + 25), including protected EOF/source UTF-16 pins | `yaml-phase1/candidate-md-pins.log` |
| `cargo test --locked -p zfb --no-default-features --lib diagnostics::tests` | Initial FAIL exit 101; recovered PASS 11/11, zero ignored | initial `yaml-phase1/candidate-diagnostics.log`; recovery `yaml-remaining/candidate-diagnostics.log` |
| Separate `cargo check --locked -p zfb-content` | PASS exit 0 | `yaml-remaining/candidate-content-check.log` |
| Separate `cargo check --locked -p zfb-md-wasm` | PASS exit 0 | `yaml-remaining/candidate-md-check.log` |
| Separate `cargo check --locked -p zfb --no-default-features` | PASS exit 0, 29 feature-gated warnings | `yaml-remaining/candidate-cli-check.log` |
| `cargo tree --locked -e normal -i noyalib`, at both pins | PASS; single core → alias → content reverse path | `yaml-remaining/{baseline,candidate}-normal-tree.log` |
| `cargo tree --locked -e features -i noyalib`, at both pins | PASS; alias std/compat closure | `yaml-remaining/{baseline,candidate}-feature-tree.log` |
| Cargo lock checksum/graph comparison before compilation | PASS exact two version/checksum entries only, **594 packages at both pins** | `yaml-phase1/candidate-lock.log`; `yaml-remaining.log` |
| `cargo deny list`, at both pins | PASS, byte-identical after normalizing only YAML versions; 15 unchanged categories, Apache-2.0 289 / MIT 432 | `yaml-remaining/{baseline,candidate}-license.log` |
| `cargo deny check`, at both pins | PASS advisories/bans/licenses/sources, existing warnings; no new exception | `yaml-remaining/{baseline,candidate}-deny.log` |
| Restore and protected hash comparison | PASS all protected files byte-identical | `yaml-remaining.log`, snapshot hash manifests |

The initial full Phase 1 guard reported **FAIL exit 101 in 226s**: the CLI build script could not download pinned esbuild after five send-error attempts. This was an environmental prerequisite failure, preserved rather than relabeled passed. Recovery used the documented `ZFB_ESBUILD_BIN` hook with the exact registry esbuild 0.25.12 archive SHA-256 `bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014`, compared against `crates/zfb/build.rs` before use (`esbuild-provenance.txt`). No production change, dependency workaround or weakened assertion was needed. The recovery guard reported **PASS exit 0 in 257s**; its nested disk probe still returned exit 1 and is recorded below as blocked.

Toolchain: Linux x86_64, rustc `1.99.0 (b940084d7 2026-09-28)` / LLVM 23.1.1; cargo `1.99.0 (5f94df478 2026-08-27)`, cargo-deny 0.19.9, Node v24.19.0, pnpm 12.8.2. Captured stable toolchain and supported task-local installation in `toolchain.txt`; only native x86_64 std is installed. There is no Mac/Windows or different-compiler observation. Frozen pnpm installation supplied unchanged CLI prerequisites. Guard minimum available-memory observations: 15,022 MiB initial / 15,185 MiB recovery; these are not peak RSS measurements.

## Phase 2 blocked without a verdict

At **2026-10-08T07:52:25.793254Z**, the exact disk check observed **14,959,755,264 free bytes**, below the unchanged **32,212,254,720 bytes (30 GiB)** minimum. The check returned exit 1. This host's 32 GiB total filesystem cannot currently admit Phase 2 after toolchain/dependency/native-target storage. No cleanup of unrelated state, alternative compiler or gate reduction was attempted.

**NOT RUN due to that gate:** wasm32 check; strict composite `pnpm test:md-wasm`; candidate and same-toolchain 0.0.44 timed four-artifact builds; sixteen-field artifact/control comparison; budget/temporary manifest-refresh checks; tarball/complete-dist measurement; twelve-step production timing; peak RSS/Phase 2 disk sampling. wasm32 std and wasm-bindgen CLI were not installed; their required versions remain additional preflights for a suitably provisioned environment. No artifact bytes, headroom or performance result is inferred from prior evaluations.

| Artifact | Unchanged gzip-9 ceiling | Current measurement |
| --- | --- | --- |
| default | 1,600,000 B | NOT RUN: disk gate |
| highlight-only | 880,000 B | NOT RUN: disk gate |
| render-only | 1,100,000 B | NOT RUN: disk gate |
| parse-only | 325,000 B | NOT RUN: disk gate |
| Tarball | 3,900,000 B | NOT RUN: disk gate |

The prior 0.0.51 KEEP's same-toolchain 0.0.44 control also exceeded render-only; that cannot permit MIGRATE here. Resume the exact ordered commands in `README.md` on a host clearing the gate, keep both compiler/control identities identical, preserve every ceiling, and repeat final release-race checks immediately before any terminal verdict. Until then the status is **BLOCKED**, with production 0.0.44 retained operationally and no unsupported KEEP/MIGRATE decision.
