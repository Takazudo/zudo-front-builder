<!-- Historical #4055 Linux Phase 1 evidence; recorded gate outcomes are preserved, with timeline context added. -->

# Historical Linux YAML 0.0.55 differential evaluation #4049 — BLOCKED at that observation

This preserves the earlier Linux 0.0.55 record. Its Phase 2 disk gate was checked at
2026-10-08T07:52:25.793254Z; the later 0.0.56 macOS evaluation and final KEEP decision below
supersede its status as the latest evaluation.

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

The initial full Phase 1 guard reported **FAIL exit 101 in 226s**: the CLI build script could not download pinned esbuild after five send-error attempts. This was an environmental prerequisite failure, preserved rather than relabeled passed. Recovery used the documented `ZFB_ESBUILD_BIN` hook with the exact registry esbuild 0.25.12 extracted binary SHA-256 `bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014`, compared against `crates/zfb/build.rs` before use (`esbuild-provenance.txt`). No production change, dependency workaround or weakened assertion was needed. The recovery guard reported **PASS exit 0 in 257s**; its nested disk probe still returned exit 1 and is recorded below as blocked.

Toolchain: Linux x86_64, rustc `1.99.0 (b940084d7 2026-09-28)` / LLVM 23.1.1; cargo `1.99.0 (5f94df478 2026-08-27)`, cargo-deny 0.19.9, Node v24.19.0, pnpm 12.8.2. Captured stable toolchain and supported task-local installation in `toolchain.txt`; only native x86_64 std is installed. There is no Mac/Windows or different-compiler observation. Frozen pnpm installation supplied unchanged CLI prerequisites. Guard minimum available-memory observations: 15,022 MiB initial / 15,185 MiB recovery; these are not peak RSS measurements.

## Phase 2 blocked without a verdict

At **2026-10-08T07:52:25.793254Z**, the exact disk check observed **14,959,755,264 free bytes**, below the unchanged **32,212,254,720 bytes (30 GiB)** minimum. The check returned exit 1. This filesystem cannot currently admit Phase 2 after toolchain/dependency/native-target storage. No cleanup of unrelated state, alternative compiler or gate reduction was attempted.

**NOT RUN due to that gate:** wasm32 check; strict composite `pnpm test:md-wasm`; candidate and same-toolchain 0.0.44 timed four-artifact builds; sixteen-field artifact/control comparison; budget/temporary manifest-refresh checks; tarball/complete-dist measurement; twelve-step production timing; peak RSS/Phase 2 disk sampling. wasm32 std and wasm-bindgen CLI were not installed; their required versions remain additional preflights for a suitably provisioned environment. No artifact bytes, headroom or performance result is inferred from prior evaluations.

| Artifact | Unchanged gzip-9 ceiling | Current measurement |
| --- | --- | --- |
| default | 1,600,000 B | NOT RUN: disk gate |
| highlight-only | 880,000 B | NOT RUN: disk gate |
| render-only | 1,100,000 B | NOT RUN: disk gate |
| parse-only | 325,000 B | NOT RUN: disk gate |
| Tarball | 3,900,000 B | NOT RUN: disk gate |

The prior 0.0.51 KEEP's same-toolchain 0.0.44 control also exceeded render-only; that cannot permit MIGRATE here. Resume the exact ordered commands in `README.md` on a host clearing the gate, keep both compiler/control identities identical, preserve every ceiling, and repeat final release-race checks immediately before any terminal verdict. Until then the status is **BLOCKED**, with production 0.0.44 retained operationally and no unsupported KEEP/MIGRATE decision.

---

# Mac Phase 1 evaluation of 0.0.56 (#4065) — GREEN native gates; verdict not yet available at Phase 1 completion

This Phase 1 record follows the preserved #4055 Linux 0.0.55 record above. It evaluates source `9375431ab81a95f480063f392b61444b9b5d892d` on macOS aarch64. The production `Cargo.toml` and `Cargo.lock` are restored to `=0.0.44`; no product source, harness, fixture, size ceiling, or watcher baseline changed. At Phase 1 completion, Phase 2 and the final KEEP/MIGRATE decision were **NOT RUN / not established**; the completed Phase 2 and decision records below supersede that point-in-time status.

## Selection and provenance

A live crates.io re-query at **2026-10-08T14:05:32Z** selected **0.0.56**, the newest version published for both crates and yanked for neither. It supersedes the historical 0.0.55 run. Downloaded archives matched registry SHA-256; GitHub Release body checksums, asset digests, and asset sizes matched those same archives. Both declare `MIT OR Apache-2.0` and Rust minimum `1.86.0`. The published alias has exact core `=0.0.56`, no default features, and `std` plus `compat-serde-yaml`. `provenance.json` records publication timestamps, checksums, bytes, skipped versions, and published manifests; `github-provenance.json` records annotated tags, dereferenced commits, Releases with checksum bodies, open PR collections, and merged states for PRs 459/28.

| Crate | Registry published UTC | SHA-256 | Archive bytes | Annotated tag / commit |
| --- | --- | --- | ---: | --- |
| noyalib | 2026-10-08T10:12:23.979942Z | `17cb8fe21481880d02e866e6678da0ce313735b659edf3dfd4beb7fdaca06e77` | 1,278,749 | `d292b8f187e2efce2bf008f9edf19eaa44a44618` / `d39c929a3d98487dea8fe51d42f1675851f6eef6` |
| noyalib-serde-yaml | 2026-10-08T10:42:11.418941Z | `bce484e3ab7c08fa9f1183382e9ff91770651ed15ad8c9aa6ef1f9badd5d6a70` | 73,236 | `61c88b9ccc2847ec8b7d8a7d797d8778f4ac34b6` / `27b152f0bc5f508178723edb5ea687545efb97db` |

The final registry/archive check at **2026-10-08T14:30:21.773110Z** passed: adopted 0.0.44 and candidate 0.0.56 were not yanked, and no newer complete pair or half-pair was present (`final-registry.log`, exit 0). This is a point-in-time observation.

## Host and controlled execution

Raw evidence: `$HOME/.cache/zfb-4049-yaml-evidence/20261008_1404/` (outside the checkout and Dropbox). `env.sh` fixes `RUSTUP_HOME=$HOME/.rustup`, a task-local `CARGO_HOME`, one shared `CARGO_TARGET_DIR`, and the rustup stable compiler bin before Homebrew. The resolved compiler was **rustc 1.96.0** (`ac68faa20`, LLVM 22.1.2) and **cargo 1.96.0** (`30a34c682`), both aarch64-apple-darwin. Homebrew's shadowing rustc/cargo 1.94.0 were not used. `preflight.log` records executable paths, rustup active toolchain, installed targets including wasm32, wasm-bindgen **0.2.121**, package binaryen wasm-opt **130**, Node **v24.14.0**, pnpm **12.8.2**, cargo-deny **0.19.9**, and Python **3.12.13**. `pnpm install --frozen-lockfile` passed (exit 0); `pnpm-lock-before.sha` and `pnpm-lock-after.sha` are identical. Initial `df` reported 78,530,760 available 1 KiB blocks; after Phase 1 the exact free-byte read was 71,490,424,832. Peak RSS was not sampled, so no peak claim is made.

The first guarded attempt used a fixed but incorrect `ZFB_RELEASE_VERSION=0.0.0-yaml-eval`. The API test `version_falls_back_to_cargo_pkg_version_in_dev_builds` correctly rejected that stamp (observed `0.0.0-yaml-eval`, expected `0.0.0`): **FAIL exit 101**, with 32/33 API tests passing before stop. This was a runner configuration error, not a candidate YAML incompatibility. The first run completed baseline normal/feature graphs, registry, exact lock verification, candidate harness 4/4, and full content 1,081 before the failure. Diagnostics, the separate checks, and candidate audit were **NOT RUN** in that attempt. The failure is preserved verbatim in `candidate-md-pins.log`, `candidate-md-pins.exit`, and `phase1-guard.log`; the guard verdict is FAIL. Its EXIT trap restored all protected hashes.

The **first and only corrective round** set a single fixed stamp, `ZFB_RELEASE_VERSION=0.0.0`, matching the development test. It used the original immutable `snapshot.json` and saved candidate lock: `resumed-pin`, `resumed-lock`, then all candidate tests/checks/graphs and `audit.sh` under one guarded `resume-phase1.sh`. It did not rerun `snapshot` or overwrite first-attempt logs. Both baseline and candidate native work used the same rustup compiler/cargo and shared target. The documented `ZFB_ESBUILD_BIN` input used an externally staged macOS arm64 esbuild 0.25.12 binary, SHA-256 `3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5`, equal to `crates/zfb/build.rs`; see `esbuild-provenance.txt`. The resumed guard verdict was **PASS exit 0**, and its EXIT trap again restored and verified protected files. No test assertion was weakened.

| Candidate check at 0.0.56 | Result | Evidence file prefix |
| --- | --- | --- |
| 18-case differential equality, 4/4 harness tests | PASS | `resumed-harness` |
| `cargo test --locked -p zfb-content` | PASS 1,081, zero failures | `resumed-content` |
| `cargo test --locked -p zfb-md-wasm --test api --test parse_to_ast` | PASS 58/58 (33 + 25) | `resumed-md-pins` |
| `cargo test --locked -p zfb --no-default-features --lib diagnostics::tests` | PASS 11/11 | `resumed-diagnostics` |
| Separate `cargo check --locked` for zfb-content, zfb-md-wasm, and zfb without default features | PASS 0/0/0; CLI emitted 29 feature-gated warnings | `resumed-content-check`, `resumed-md-check`, `resumed-cli-check` |
| Normal and feature reverse dependency trees | PASS; identical to baseline after normalizing only the two YAML versions | `baseline-*-tree`, `resumed-*-tree` |
| Both-pin `cargo deny list` and `cargo deny check` | PASS/0 at both pins; advisories, bans, licenses, sources all okay | `baseline-*`, `candidate-*` audit files |
| Snapshot restoration and all protected hashes | PASS after first and resumed attempts | `phase1-guard.log`, `resume-guard.log`, final `evaluate.py verify` |

The 18 unchanged corpus observations all matched; the historical table above lists each case. P3 is not applicable. The lock had **594 packages at both pins**, with exactly two changed entries: `noyalib` and `noyalib-serde-yaml`, each 0.0.44 → 0.0.56 with its frozen checksum; all other lock entries were byte-equivalent. Both `cargo deny list` outputs were byte-identical after normalizing only the two YAML versions: 15 license categories, including Apache-2.0 289 and MIT 432. `cargo deny check` passed advisories/bans/licenses/sources at both pins with existing warnings, and no exception was added. There were **zero production lines changed and zero source corrective rounds**; the runner stamp fix was one corrective round for this evaluation.

Every command log and child `.exit` lives in the evidence directory. Cheap verification passed: Python syntax, Bash syntax, JSON parsing, registry assertion, pnpm lock hash comparison, exact lock delta, `git diff --check`, and final protected-hash check. The original Phase 1 guard and corrected guard verdicts remain distinct. At Phase 1 completion, Phase 2 sizes, the same-toolchain wasm control, CI ceiling validation, and terminal KEEP/MIGRATE were **NOT RUN**; the Phase 2 record below subsequently completed the local measurements, and the decision record below resolves the verdict.

---

# Mac Phase 2 evaluation of 0.0.56 (#4066) — COMPLETE evidence, strict size FAIL

**Phase 2 status: COMPLETE with evidence.** The candidate's strict `pnpm test:md-wasm`, timed production wrapper, and budget assertion all failed at the unchanged render-only gzip-9 ceiling. The same-toolchain 0.0.44 control passed that ceiling. This is a real candidate gate failure on this host, not a Mac/control shared overage. **Verdict: KEEP — retain production noyalib / noyalib-serde-yaml =0.0.44; evaluated 0.0.56 is rejected for same-toolchain size growth.** No product pin, ceiling, CI size manifest, or documentation size table was changed.

Raw evidence is in `$HOME/.cache/zfb-4049-yaml-evidence/20261008_1404/`; the compact machine-readable result is `phase2-summary.json`. Tested source SHA was the merged Phase 1 base `8d811be3990ca560135f57aaf8705bb36f9bd05f`. This run used rustc/cargo 1.96.0 from `stable-aarch64-apple-darwin`, wasm-bindgen 0.2.121, binaryen wasm-opt 130, Node v24.14.0, and pnpm 12.8.2. Host: macOS 26.6.1, Apple A18 Pro. The same `env.sh`, `CARGO_HOME`, `RUSTUP_HOME`, pinned PATH, `ZFB_RELEASE_VERSION=0.0.0`, and package dependencies were used for both pins. The two timed targets were absent before their builds and were kept separate. All Rust/wasm build commands ran serially inside the machine-wide heavy guard; it reported `verdict=PASS exit=0 secs=599` for the *runner*, not for each child gate.

## Ordered commands and gates

The runner saved every command, log, real child exit, start/end timestamp, disk snapshot, and `/usr/bin/time -l` result as `phase2-<name>.*`. Its EXIT trap restored `Cargo.toml`, `Cargo.lock`, and `shipped-sizes.json` then ran hash verification, both exit 0. `pin` → saved candidate lock copy → `lock` passed with exactly two version/checksum entries, 594 packages; `disk` passed at **73,649,041,408 free bytes** against **32,212,254,720**. The post-restore control disk recheck passed at **71,899,529,216 free bytes**. There was no gate reduction.

| Sequence | Command / evidence name | Actual exit | Result |
| --- | --- | ---: | --- |
| 1 | `cargo check --locked --target wasm32-unknown-unknown -p zfb-md-wasm` / `candidate-check` | 0 | PASS |
| 2 | `pnpm test:md-wasm` / `candidate-strict` | 1 | **FAIL**: `render-only gzip-9 size 1111923 exceeds ceiling 1100000`; composite stopped before its test half |
| 3 | `CARGO_TARGET_DIR=$YAML_EVIDENCE/wasm-candidate node scripts/run-zfb-md-wasm-build-timed.mjs` / `candidate-build` | 1 | All four artifact passes and twelve subprocess timings emitted; final wrapper total unavailable because production build threw on the same ceiling |
| 4 | `node scripts/assert-zfb-md-wasm-budgets.mjs --build-log $YAML_EVIDENCE/candidate-build.log --dist crates/zfb-md-wasm/npm/dist --update-manifest` / `candidate-budget` | 1 | **FAIL**, same exact render-only ceiling; no CI manifest update retained |
| 5 | `evaluate.py restore`, tool identities, and `disk` / `restore`, `control-identities`, `control-disk` | 0 each | Original 0.0.44 pin; unchanged tool versions and disk gate |
| 6 | `CARGO_TARGET_DIR=$YAML_EVIDENCE/wasm-control node scripts/run-zfb-md-wasm-build-timed.mjs` / `control-build` | 0 | PASS, four artifacts and twelve timings |
| 7 | Same strict budget assertion on `control-build.log` / `control-budget` | 0 | PASS, all four gzip-9 ceilings |
| 8 | Final `evaluate.py restore`, `verify`, and EXIT restore/verify | 0 each | All protected hashes byte-identical; production YAML pin still `=0.0.44` |

The independent `pnpm --filter @takazudo/zfb-md-wasm test` on saved candidate dist passed **234/234 in 14 files** (exit 0; `phase2-candidate-tests.log`). It supplies diagnostic test evidence and does not change the strict composite FAIL. Candidate and control `pnpm pack --pack-destination $YAML_EVIDENCE` both exited 0, from their respective saved dist; neither package was published. All tarball outputs remain outside the checkout.

## Actual shipped artifact bytes

Each field was measured from its saved `candidate-dist` or `control-dist` file using Node gzip level 9, independent of the budget script's exit, and final wasm was cross-checked against each build log. `Δ` is candidate minus control in bytes. The gzip-9 ceiling applies to the gzip-9 column; headroom is ceiling minus observed gzip-9 bytes. The glue columns have no separate absolute ceiling.

| Entry | Field | Control 0.0.44 | Candidate 0.0.56 | Δ |
| --- | --- | ---: | ---: | ---: |
| root | finalWasm | 3,398,523 | 3,436,050 | +37,527 |
| root | gzip9 | 1,524,882 | 1,541,225 | +16,343 |
| root | glue | 14,998 | 14,998 | 0 |
| root | glueGzip9 | 4,199 | 4,199 | 0 |
| highlight | finalWasm | 1,537,137 | 1,537,137 | 0 |
| highlight | gzip9 | 822,289 | 822,285 | −4 |
| highlight | glue | 8,758 | 8,758 | 0 |
| highlight | glueGzip9 | 2,637 | 2,637 | 0 |
| render | finalWasm | 2,204,458 | 2,240,279 | +35,821 |
| render | gzip9 | 1,098,011 | **1,111,923** | **+13,912** |
| render | glue | 8,772 | 8,772 | 0 |
| render | glueGzip9 | 2,661 | 2,661 | 0 |
| parse | finalWasm | 704,576 | 741,625 | +37,049 |
| parse | gzip9 | 283,410 | 298,857 | +15,447 |
| parse | glue | 11,159 | 11,159 | 0 |
| parse | glueGzip9 | 3,797 | 3,797 | 0 |

| Complete output / gate | Ceiling | Control | Control headroom | Candidate | Candidate headroom | Δ |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| root gzip-9 | 1,600,000 | 1,524,882 | +75,118 | 1,541,225 | +58,775 | +16,343 |
| highlight gzip-9 | 880,000 | 822,289 | +57,711 | 822,285 | +57,715 | −4 |
| render gzip-9 | 1,100,000 | 1,098,011 | +1,989 | **1,111,923** | **−11,923** | +13,912 |
| parse gzip-9 | 325,000 | 283,410 | +41,590 | 298,857 | +26,143 | +15,447 |
| Complete dist bytes | — | 8,133,458 | — | 8,243,855 | — | +110,397 |
| Packed tarball bytes | 3,900,000 | 3,814,132 | +85,868 | 3,859,716 | +40,284 | +45,584 |

The clean candidate production subprocesses summed to **174.055 s** from the twelve emitted lines; the wrapper did not print its final total because the ceiling threw. Control printed **169.402 s**; summing its rounded twelve lines gives 169.403 s. The difference of the summed lines is **+4.652 s**; the 210-second Apple-M4 reference is informational here. Total timed-command wall time was **176.55 s candidate / 171.49 s control**. Per artifact, the production triplets (cargo rustc / wasm-bindgen / wasm-opt, seconds) were candidate **95.521 / 0.320 / 11.003**, **31.178 / 0.116 / 3.736**, **14.666 / 0.152 / 6.170**, **8.747 / 0.050 / 2.396**; control **95.769 / 0.204 / 10.460**, **27.458 / 0.077 / 3.857**, **13.395 / 0.107 / 6.687**, **8.554 / 0.098 / 2.737** (root, highlight, render, parse order). `/usr/bin/time -l` observed wrapper maximum resident sizes of **615,497,728 / 591,298,560 bytes**. It did not capture a reliable aggregate peak RSS across all child processes, so aggregate peak RSS is **unavailable**. The 28 before/after filesystem snapshots ranged from **73,649,192,960** to a low of **71,342,694,400** available bytes; the latter is a sampled disk high-water point, not a continuous process peak. The two exact disk-gate reads are also in evidence.

## Dependency audit and final release race

The Phase 1 saved normal and feature reverse trees and `cargo deny list` category outputs compare byte-identically after normalizing only `0.0.44`/`0.0.56` YAML version strings; each `cmp` exit was 0. Both-pin `cargo deny check`, tree, and list exits were 0. The shared lock comparison found 594 packages and exactly `noyalib` and `noyalib-serde-yaml` version/checksum changes; no audit exception or source change was introduced. The license list retains 15 categories (Apache-2.0 289, MIT 432), as in Phase 1.

`evaluate.py registry` checked at **2026-10-08T14:46:54Z**, exit 0: both adopted 0.0.44 and candidate 0.0.56 remained non-yanked; actual 0.0.56 archives/checksums and the alias exact dependency matched frozen provenance; no newer complete or half-pair existed. The full 0.0.45–0.0.55 per-crate publication timestamps and non-yanked states are retained in `phase2-final-registry.log` (all 11 intermediate versions non-yanked for both crates). Release-race row (a): **0.0.56 selection still current; no re-selection needed at this observation**.

Authenticated GitHub reads at **2026-10-08T14:47:09Z** rechecked both `v0.0.56` refs, annotated tag objects, dereferenced commits, Release publication/body/checksum assets, archived state, and complete open-PR collections. All matched `github-provenance.json`, neither repo was archived, and both open-PR collections were empty (`phase2-final-github.json`, exit 0). The authenticated drift detector `GITHUB_TOKEN=$(gh auth token) node scripts/check-yaml-candidate-drift.mjs --json` ran at **2026-10-08T14:46:57Z** and returned **exit 10 / `CANDIDATE_DRIFT`**, with no errors. Its deltas compare the committed older watcher baseline with releases 0.0.52–0.0.56, tags, branches, and merged release PRs; they do not contradict the direct frozen 0.0.56 race check. The detector JSON and stderr were saved without writing any snapshot onto the committed baseline.

The final `evaluate.py verify` and EXIT verification passed; `git status` had only this report and `phase2-summary.json` as intended changes. The candidate's strict absolute size gate remains red while the same-toolchain control passes. The terminal KEEP decision is recorded below; no local MIGRATE qualification is asserted.

## Final decision, race check, and KEEP application (#4067/#4068)

**Verdict: KEEP — retain production noyalib / noyalib-serde-yaml =0.0.44; evaluated 0.0.56 is rejected for same-toolchain size growth.** Issue #4067 recorded the reviewed decision on merged evidence SHA `9d1b92e225a1904bf74f87940f9c6c5559570f39`; #4068 applies that verdict. This is a final rejection of 0.0.56, not a pending-CI migration or a blocked evaluation.

The strict render-only gzip-9 ceiling is 1,100,000 B. The 0.0.44 same-toolchain control measured 1,098,011 B and passed; 0.0.56 measured 1,111,923 B and failed by 11,923 B. The accepted growth limit is at most 1,024 B for each artifact's `finalWasm` and `gzip9`, with zero render growth. Root, render, and parse growth exceed 1,024 B; render also violates zero growth. CI's 6,027 B headroom does not permit projecting these Mac deltas onto CI. The complete artifact and ceiling tables above remain the source of the byte counts. Diagnostic wasm tests passed 234/234 separately and do not convert the strict composite FAIL to PASS.

**GHSA-4xcc-23fx-w2wj non-exposure:** zfb does not use `ParserConfig` or YAML budget setters in `crates/`; its YAML path deserializes with `serde_yaml::from_str` into `Value` (explicit `::<Value>` in the differential harness). This call-path observation is not a claim that the upstream advisory is fixed.

The instructed call-site search, `rg -n 'ParserConfig|set_.*budget|with_.*budget|serde_yaml::from_str' crates --glob '*.rs'`, returned two matches, both inspected: `crates/zfb-content/src/frontmatter.rs:254` calls `serde_yaml::from_str(yaml_str)`; `crates/zfb-content/tests/yaml_differential_harness.rs:112` calls `serde_yaml::from_str::<Value>(yaml)`. No `ParserConfig` or budget-setter call site matched.

### Final registry and GitHub race check

The fresh `evaluate.py registry --evidence` read started at **2026-10-08T15:06:38Z**, reported `checkedAt = 2026-10-08T15:06:38.359015+00:00`, and exited 0. Both the adopted 0.0.44 versions and candidate 0.0.56 versions remained non-yanked. The 0.0.56 downloaded archive SHA-256 values matched the registry checksums and frozen provenance: core `17cb8fe21481880d02e866e6678da0ce313735b659edf3dfd4beb7fdaca06e77`, alias `bce484e3ab7c08fa9f1183382e9ff91770651ed15ad8c9aa6ef1f9badd5d6a70`. The published alias manifest still requires exact core `=0.0.56`, default features false, with `std` and `compat-serde-yaml`. Neither crate had a newer non-yanked version; `newerCompletePairs` was empty. All passed-over 0.0.45–0.0.55 versions were non-yanked at this observation:

| Pair | Core published UTC | Alias published UTC | Yank state |
| --- | --- | --- | --- |
| 0.0.45 | 2026-09-18T18:05:28.349263Z | 2026-09-19T07:41:27.464727Z | both non-yanked |
| 0.0.46 | 2026-09-21T15:04:50.854459Z | 2026-09-21T22:31:50.381115Z | both non-yanked |
| 0.0.47 | 2026-09-21T22:54:39.552783Z | 2026-09-21T23:10:08.420790Z | both non-yanked |
| 0.0.48 | 2026-09-22T00:59:43.932787Z | 2026-09-22T07:23:53.360055Z | both non-yanked |
| 0.0.49 | 2026-09-22T09:31:12.915663Z | 2026-09-22T09:49:53.603529Z | both non-yanked |
| 0.0.50 | 2026-09-22T12:02:45.101054Z | 2026-09-22T12:21:53.235495Z | both non-yanked |
| 0.0.51 | 2026-09-22T14:47:58.355202Z | 2026-09-22T15:04:44.656167Z | both non-yanked |
| 0.0.52 | 2026-10-04T14:20:19.747688Z | 2026-10-06T17:23:42.884763Z | both non-yanked |
| 0.0.53 | 2026-10-06T12:41:27.063308Z | 2026-10-06T18:20:53.416765Z | both non-yanked |
| 0.0.54 | 2026-10-07T14:07:29.154401Z | 2026-10-07T14:20:18.633508Z | both non-yanked |
| 0.0.55 | 2026-10-08T06:37:46.465213Z | 2026-10-08T07:10:21.924189Z | both non-yanked |

Authenticated complete open-PR collections were read at **2026-10-08T15:06:48Z–15:06:49Z**, both exit 0 and both `[]`: `sebastienrousseau/noyalib` and `sebastienrousseau/noyalib-serde-yaml`. No open release PR needs a watcher pointer. The subsequent authenticated pre-refresh detector at `2026-10-08T15:06:55.654Z` exited **10**, `CANDIDATE_DRIFT`, with `errors: []`. Its 32 adopted-pair triage deltas are the 30 already-evaluated publish/tag/Release deltas for 0.0.52–0.0.56 plus the merged-state transitions for core PR #459 and alias PR #28. The remaining branch movement, including fallback-candidate movement, is informational. Exit 10 is this expected old-baseline drift report, not a compatibility-test failure.

The complete captured stdout, stderr, timestamps, actual exit files, and registry result are outside the checkout at `/tmp/zfb-4068.jxyFRM/`. The watcher pointers are therefore both `null`; this is supported by the complete fresh open-PR results, not inferred from branch names. One snapshot refresh was performed: authenticated `--snapshot` started at **2026-10-08T15:10:55Z**, exited 0, wrote `/tmp/zfb-4068-baseline.M6xsW4/snap.json`, and was copied once to `scripts/yaml-candidate-baseline.json`; the targeted formatter exited 0. The formatted baseline has `checkedAt = 2026-10-08T15:10:56.693Z`, and both release-PR pointers are `null`. The authenticated post-refresh detector at **2026-10-08T15:11:09.004Z** exited 0 with status `no-drift` and `errors: []`; all seven candidates had no deltas, including no informational deltas. No second snapshot was made.

### Detector regression-test correction and final verification

The exact focused command was `pnpm exec vp test run --project scripts scripts/__tests__/check-yaml-candidate-drift.test.mjs`. Its initial pre-refresh run had **75/77 passing and 2 failing**; that invocation was diagnostic while the old baseline still held OPEN PR pointers and the updated detector config held null. The failures were the obsolete expectation that PR #459/#28 would be queried and an expected temporary baseline/config pointer mismatch. After the single refresh, the unchanged tests still had **75/77 passing and the same 2 stale assumptions**: hardcoded PR #459/#28 requests and hardcoded non-null config values.

The test correction now asserts null pointers, zero PR requests, and null observations against the actual current configuration; an adjacent positive test exercises both PR fetches with synthetic pointers 999/1000 and restores both pointers in `finally`. The canonical baseline/config equality assertion remains intact, as do OPEN → MERGED/CLOSED detection, malformed-response handling, and operational-failure coverage. The final suite passed **78/78**. `node --check scripts/__tests__/check-yaml-candidate-drift.test.mjs`, `node --check scripts/check-yaml-candidate-drift.mjs`, `pnpm format:check`, and `git diff --check` all exited 0. The diff contains only the five authorized files; Cargo pins/lock, harness and corpus, fixtures, size manifest, size documentation, and ceilings match the incoming base. No browser or visual check applies.
