# Rolldown findings — 2026-10-15

**The isolated plugin alias defect is repaired and verified; keep the live plugin caller on esbuild.** The expanded plugin option probe now passes 18 contracts on each real backend. This is a bounded Linux capability result, not production plugin acceptance or permission to adopt Rolldown by default. PR #4120 remains open and draft.

## Whole-PR experimental integration

[The integration review](INTEGRATION-20261015.md) records the merge of actual main
`57f625a2`, the main-seeded Cargo resolution experiment, shared producer impact,
three-configuration verifier and automatic merge workflow effects. The direction
is an explicit experimental opt-in for dogfooding; esbuild remains the default.

Whole-PR acceptance is blocked by seven retained prepared-contract assertions
(six trailing-directory resolution cases and unsupported raw CSS bundling),
the nonadditive Cargo provenance transition
and a newly measured render-WASM gzip result of 1,100,087 bytes against the
unchanged 1,100,000 ceiling. The older provenance-only failures below did not
establish this new numerical regression. No held policy, contract digest or
ceiling is changed. Required CI, independent runtime verification and the
owner's final merge/deployment authorization remain distinct.

## Supervision increment from `8cd83962`

[Owned supervision and cancellation](PLUGIN-CANCELLATION.md) now documents the
CLI's bounded admission, non-blocking request drop, independent cancellation
deadlines, fixed join lanes, and explicit unbounded root drain. Actual join
remains the publication boundary. The 16 new supervisor regressions run beside
all 26 existing example tests and the unchanged 36-row differential matrix.
Final-head run evidence belongs to the PR report; historical runs below retain
their own source identities.

Independent review reproduced `is_finished()` preceding blocked TLS destruction;
fixed join lanes avoid head-of-line blocking without weakening actual join.
Pinned source also shows global Rayon deferred destruction after generation,
with its private drain at next build entry. Owned-job drain is therefore not
universal Rolldown quiescence, and hard bounded native termination is still
unproved. Production staging/caller adoption stays gated. No pin, dependency,
lockfile, generated artifact or production backend changed in this increment.

## Earlier alias implementation and execution

The native example now delegates local mappings to Rolldown's own resolver before externalizing ordinary unresolved packages. It preserves genuinely external dependencies, including entry-adjacent nested-package lookup. It does not hardcode aliases, implement a second tsconfig resolver, classify every node_modules path as external, or convert configuration/package-import failures into successful external imports.

[Plugin alias verification](PLUGIN-ALIAS-VERIFICATION.md) records the complete matrix, source and artifact identities, exact lock change, genuine Wind regeneration, reproduction commands and limitations. [Run 38634288237](https://github.com/Takazudo/zudo-front-builder/actions/runs/38634288237) completed successfully on Linux x86_64 with Rust 1.99.0 and Node 24.14.0. The reviewed source tree is `13a0cdab0c5d71d09427e1bc67984117630ab949`; integration adds these reports and retires the temporary collector without further Rust, lock or generated-asset changes.

| Check | Actual result |
| --- | --- |
| Real esbuild/native plugin differential execution | 18 + 18 passed; zero failures |
| Native example unit tests | 2 passed |
| Native prototype library tests | 4 passed |
| Feature-off prototype all-target check | Passed |
| Locked no-V8 zfb build | Passed; ordinary graph contains no Rolldown |
| Wind library tests | 117 passed |
| Wind preview-pipeline tests | 19 passed; none skipped |
| Genuine compiled previews and freshness | 173 samples passed; no CSS-byte changes |
| English and Japanese reference freshness | Both passed |
| Unreviewed Wind pin and tampered-lock rejection | Both retained and passed |

The probe retains all eight original contracts and adds inherited aliases, baseUrl, package-shadowing aliases, aliases into node_modules, dynamic and CommonJS aliases, late-installed packages, package imports and malformed-input failures. Changing real dependencies after bundling proves the emitted modules did not accidentally inline those packages.

Only one root dependency entry is added to Cargo.lock: the prototype exposes the already-pinned optional `rolldown_resolver` dependency for typed resolution errors. No crate version or Rolldown revision changes. The exact Rolldown revision remains `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`. Wind provenance is refreshed after auditing the exact changed source closure and executing actual tests/previews; compatibility support pins, catalogs, assertions and budgets are unchanged.

## Earlier evidence remains source-specific

The [previous findings at cb366ea9](https://github.com/Takazudo/zudo-front-builder/blob/cb366ea9691c2583e354a1fef4e806684e9523ba/prototypes/rolldown/FINDINGS.md) retain the original esbuild 8/8 versus native 7/8 alias failure, its emitted-module diagnosis and prior CI observations. That failure is not reclassified as a pass. The current correction supersedes its next-step wording.

[POST-SYNC.md](POST-SYNC.md) retains the earlier main synchronization, existing native client/browser/SSR execution and generated-data repair evidence at their named sources. Main `fbb7921d8fb3f28d859bbd4cf5737759f547866a` was incorporated by `557f65e3`; the existing broad native verification ran at `21863e45`, and data repairs were committed at `9b47cfe5`. The latest isolated plugin run does not repeat every broad browser/SSR test or remeasure WASM. Earlier Darwin/WSL evidence and [historical findings](FINDINGS-HISTORY-20261012.md) remain separate. No benchmark result is changed or new performance claim made.

## Remaining gates and next implementation boundary

The production plugin subprocess has deadlines, termination behavior, entry-adjacent staging and file-ownership protections. Native close after successful completion does not establish cancellation of active work, bounded quiescence, or absence of writes after staged files are removed. The current production adapter synchronously joins its scoped thread. Those lifecycle contracts remain unimplemented and unverified for native plugin bundling.

The next implementation must independently prove cancellation/quiescence and stage ownership before connecting the real plugin caller, then execute actual CLI-to-plugin-host round trips, failure cleanup and repeated-start cases. Additional resolver cases and platform coverage must be retained as explicit obligations rather than inferred from these 18 contracts. The Windows-specific rooted-path test is present but was not executed by this Linux run.

The island-size Cargo-provenance admission remains a separate integration gate. Earlier Health failures rejected changed producer inputs, not a demonstrated numeric size overflow. This increment does not change island contracts, admission policy, ceilings, support pins or qualifying-platform requirements. Ordinary final-head CI must be read independently; a successful bounded collector is not a declaration that all required workflows are green.

The live `bundle_plugin_entry`, shared native adapter, configuration loader, Node plugin host, default backend and independent Deno-exit work remain unchanged. Keep #318/#4098/#4123/#4124/#4133 open. No ready transition, merge into main, release, publication or deployment is included.
