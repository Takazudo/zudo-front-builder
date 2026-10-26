# Reviewed Cargo producer transition for #318 / PR #4120

This decision authorizes one non-additive producer-lock proof. It does **not** admit current measurements, change numerical ceilings, repin either platform contract, or activate held #4234. The ordinary budget checker is unchanged. The root implementation manager owns final verification and native evidence reconciliation under [the development policy](../../../CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions).

The owner approved this narrow prerequisite after the whole-PR integration review at `adb507f7d3ed2de412c603f63cec6e38424dcadb`. The prior additive-only refusal remains valid history. The policy now recognizes only the exact pair below, with no general option to ignore changed dependencies.

| Input | Records | Whole-file SHA-256 |
| --- | ---: | --- |
| Historical main producer lock | 601 | `2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e` |
| Reviewed prototype producer lock | 696 | `13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37` |

[rolldown-native-318.json](rolldown-native-318.json) enumerates **111 new identities, 46 changed records, and 16 removed identities** with before/after record hashes. The old lock is preserved verbatim in [rolldown-native-318.old.lock](rolldown-native-318.old.lock), and the reviewed new lock in [rolldown-native-318.new.lock](rolldown-native-318.new.lock) (byte-identical to `adb507f7d3ed2de412c603f63cec6e38424dcadb:Cargo.lock`, read with replacement objects disabled), so policy tests do not depend on shallow checkout history or on the candidate's live `Cargo.lock`. Proving this historical pair never admits a later live lock; that remains the live contracts' own gate. The implementation pins this exact pair, validates the complete record inventory, refuses duplicates/header/other-section changes, and pins the approved manifest byte digest `e4b89f02bea8ed6eace98023c08561ff9de2289a65d7fd5b14e0e3d6c1d008ed` in code. That digest binds the feature-graph identities and review rationale too; valid-shaped invented replacements and even unreviewed formatting changes are rejected. Future rebaseline evidence records the same digest. An unreviewed next lock is rejected even if an operator changes the artifact hash and command-line hash to agree with it.

## Shared resolutions and incidental Windows edges

Fourteen removals are the shared version replacements below; `papaya@0.2.4` and `seize@0.5.1` disappear with the resolver's new dependency structure. The minima were inspected against pinned Rolldown `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9` and its resolved manifests. The [integration report](../../../prototypes/rolldown/INTEGRATION-20261015.md#cargo-resolution-evidence) preserves the main-seeded Cargo resolution reproduction, including failure at old rustc-hash and oxc_resolver minima and successful byte-identical reproduction after the narrow unlock. This decision does not change a version/source pin or claim that every selected patch is itself a minimum.

| Shared package | Historical | Reviewed | Required minimum |
| --- | --- | --- | --- |
| bitflags | 2.11.1 | 2.13.2 | 2.13.0 |
| fast-glob | 1.0.1 | 1.1.2 | 1.1.0 |
| json-escape-simd | 3.1.0 | 3.1.2 | 3.1.1 |
| memchr | 2.8.0 | 2.8.3 | 2.8.2 |
| oxc_resolver | 11.19.1 | 11.24.3 | 11.24.3 |
| regex | 1.12.3 | 1.13.1 | 1.13.0 |
| regex-automata | 0.4.14 | 0.4.18 | 0.4.16 via new regex |
| regex-syntax | 0.8.10 | 0.8.11 | 0.8.11 via new regex |
| rustc-hash | 2.1.2 | 2.1.3 | 2.1.3 |
| serde_json | 1.0.149 | 1.0.151 | 1.0.150 |
| simd-json | 0.17.0 | 0.18.1 | 0.18.0 via resolver |
| smallvec | 1.15.1 | 1.16.3 | 1.16.0 |
| unicode-segmentation | 1.13.2 | 1.13.3 | 1.13.3 via OXC diagnostics |
| uuid | 1.23.1 | 1.28.0 | 1.27.0 |

Nine surviving records reselect Windows edges. They reproduce under main-seeded resolution and are compatible with the parent manifests; they are **incidental solver choices, not native minimum requirements**. Cargo has no per-parent edge update. Keeping this exact reproduced lock avoids inventing a hand-assembled lock or reopening unrelated resolution. Acceptance of the structural transition does not prove Windows runtime behavior; native Windows/CSS verification remains a separate gate owned by the integration manager.

| Parent record | Historical edge | Reviewed edge |
| --- | --- | --- |
| anstyle-query@1.1.5 | windows-sys 0.61.2 | windows-sys 0.60.2 |
| anstyle-wincon@3.0.11 | windows-sys 0.61.2 | windows-sys 0.60.2 |
| errno@0.3.14 | windows-sys 0.61.2 | windows-sys 0.52.0 |
| is-terminal@0.4.17 | windows-sys 0.61.2 | windows-sys 0.52.0 |
| nu-ansi-term@0.50.3 | windows-sys 0.61.2 | windows-sys 0.59.0 |
| rustix@1.1.4 | windows-sys 0.61.2 | windows-sys 0.52.0 |
| socket2@0.6.3 | windows-sys 0.61.2 | windows-sys 0.60.2 |
| tempfile@3.27.0 | windows-sys 0.61.2 | windows-sys 0.52.0 |
| winapi-util@0.1.11 | windows-sys 0.61.2 | windows-sys 0.52.0 |

The other 37 changed records comprise 32 dependency-name disambiguations, the added optional `zfb-rolldown-prototype` edge in `zfb-build` and `zfb-islands`, and three expanded dependency unions: `derive_more-impl` gains `unicode-xid`, `flate2` gains `zlib-rs` while retaining `miniz_oxide 0.8.9`, and `tracing-subscriber` gains `serde`, `serde_json`, and `tracing-serde`. These are explicitly recorded, not classified as unchanged package bytes. The 111 additions include the pinned native Rolldown/OXC family and its dependency closure; the manifest enumerates every identity.

## Reviewed producer feature graphs

The existing verifier collected locked `normal,build,features` graphs on **Linux x64 / Rust 1.99.0** at the exact review source above. Full graphs are retained in `target/rolldown-integration-evidence/{default,no-v8,native}-graph.txt` and in that verifier's artifact when uploaded. These hashes identify those observations; they are not Darwin measurements and do not make later source graphs automatically verified.

| Mode | Command | SHA-256 |
| --- | --- | --- |
| default | `cargo tree --locked -p zfb -e normal,build,features` | `406840480b6ebe9fc3bb07ffe5d09e5b69e00fe9f93a30b72f8c251c689e7fc2` |
| no-v8 | `cargo tree --locked -p zfb --no-default-features -e normal,build,features` | `36064d665e23098d440a4e339d92cfe7744ea6115661bcdfd8a1aae9f5ceb2ac` |
| native | `cargo tree --locked -p zfb --features rolldown-prototype -e normal,build,features` | `6d7fdacc79519255b3449a5acbe1da8143dcca9ebe405ce963b9aac6ccca1428` |

Both ordinary graphs exclude Rolldown. They nevertheless resolve `oxc_resolver 11.24.3`, `serde_json 1.0.151`, `bitflags 2.13.2`, and `smallvec 1.16.3`; an absent optional backend does not preserve the normal producer closure. Default V8 remains `deno_core 0.399.0` / `v8 147.4.0`; the no-V8 graph omits them. Ordinary `flate2` selects the Rust backend and ordinary `derive_more` selects add/add_assign. Explicit native compilation additionally selects `flate2/zlib-rs`, `derive_more/debug`, and resolver pnp/yarn_pnp features. The final integration verifier must collect all three graphs again for its final source and must test the actual ordinary producer separately from a native-feature-unified binary.

## Native measurement gate and handoff

**Both live contracts now pin the reviewed Cargo hash `13956…` through this transition, from native evidence recorded below and in [the size README](../README.md).** The two live policy tests validate historical synthetic fixture identities, prove the reviewed structural transition, and require the three current-lock provenance failures only while a contract still pins the historical hash. Policy-test success alone never meant shipped sizes passed; the native artifacts and the strict checker carry that evidence.

Recorded 2026-10-15 (all raw/`gzipNormalized` totals, ceilings and zero allowances unchanged in every stage):

| Stage | Source | Run / job / artifact | ZIP SHA-256 (recomputed locally) | Original checker |
| --- | --- | --- | --- | --- |
| Linux transition | PR merge `070d926e24fe885f04e4be70af8525a27f9e1c79` (tree-identical to head `3f98944e427e4555ad7b13564d94857680cfb61d`) | health 38721498360 / 116378845998 / 11967322344 | `9688fb0a26b483c4aa30f2445c528c08d66a191893e0c6d013e734ed4650679a` | exit 1, only the three Cargo provenance lines |
| Darwin stage 1 (`release-bump`) | main `57f625a2e3fc4bf69eb526c7bb8ef2103d4669d6`, isolated checkout | 38729255517 / 116402683813 / 11968814824 | `bc9b17cf628f334c88b589cbb402b89ae00e5908961e750618ee40ccd24a64f9` | exit 0 |
| Darwin stage 2 (`cargo-producer-lock`) | `finalSHA` `42b1463f1b1e0f608997ed86e42bb1f0dbfdd8fd`, read back from remote | 38730893656 / 116408109872 / 11970467336 | `9c4fa84e082dec01f4bbf8aafe6c7af6753673401e9ed5ce0dad474e24253a8e` | exit 1, only the three Cargo provenance lines |

Both Darwin runs were native macos-15 arm64 (macOS 15.7.9) with Rust 1.99.0, Node 24.14.0, zlib 1.3.1-e00f703, pnpm 12.8.2 and esbuild 0.25.12. A path-rebased strict replay of the stage-2 bytes (sidecars regenerated by the pinned normalizer, totals identical to CI's) passes against the re-pinned contract and fails exactly the three Cargo lines against the stage-1 contract. The Linux checker binds to its host platform and runner package trees, so the final-head Health run is the authoritative strict Linux replay. Neither platform's evidence proves Windows behavior.

Darwin has a release-identity prerequisite: its live contract records package version **4.5.0** and pnpm lock `56e687fe05e0b84949881583381551b060cd357f43b0bf6b6bf0b27c2cc95b9a`, while this checkout and the Linux contract record **4.5.2** and `01972f66763ceaf7c0d526c43c0c0084de64f7cb5cea99b71ef9f79bf2789e0b`. The existing Cargo-only rebaseline rejects those differences. The existing `release-bump` reason can reconcile them first using a fresh native measurement of incorporated main **`57f625a2e3fc4bf69eb526c7bb8ef2103d4669d6`**, which already has version 4.5.2 and the current pnpm lock but retains Cargo `2726…`. No composed policy is needed if both stages pass all existing gates. This updates measurement evidence only: no product version change, release, package publication, or deployment is part of either stage.

Read-only identity verification confirmed that incorporated main's seven fixture hashes and both runner hashes equal the Darwin contract, its fingerprint helper is `c1cec11ab7e7d0ea5d5a9548a871dfb886bc05ebbb2ff513d13332429632d787`, and its dependency fingerprint remains `e0c6472838fa1f0f7028393c3717a2fa4942fb51ae7cf691146bc535d95f8e95`. Its combined normalizer/inventory digest is the pinned `ab60382e3c90c7417fc2fbee7429cea8281e90f0d8c8d16a5ecdbccb2e6ae71a`. These inputs also equal this candidate's; native measurements must still prove the actual totals.

The handoff below was executed on 2026-10-15 (evidence table above). The procedure is:

1. Collect the existing read-only workflow's native Darwin arm64 artifact at full source `57f625a2e3fc4bf69eb526c7bb8ef2103d4669d6`, with `platform=darwin-arm64` and `enforce=false`. Preserve its actual run/job/artifact identities, ZIP digest, saved trees, runner provenance and original checker outcome.
2. In an **isolated checkout of that exact main source**, run `scripts/rebaseline-island-size-darwin.mjs --reason release-bump` with that artifact's real `--artifact`, `--run-id`, `--job-id`, `--artifact-id`, `--artifact-expires-at`, and `--source-sha 57f625a2e3fc4bf69eb526c7bb8ef2103d4669d6`. The CLI checks the artifact against its checkout, so running this stage in the current Cargo `13956…` checkout would correctly fail. Do not temporarily replace the active branch's lock or bypass `checkoutMismatches`.
3. Review and integrate only the generated evidence updates: the Darwin `decision.json`, its rewritten test pins, and the appended README evidence section. Preserve this branch's policy code and other test changes. Stage 1 must leave Cargo pinned at `2726…`, every raw and normalized ceiling unchanged, and zero allowances. It brings the recorded package/pnpm identities to 4.5.2/current pnpm.
4. After the root manager commits and pushes the intended final candidate, **read its full SHA back from remote `prototype/318-rolldown-native`**, or use the full final SHA explicitly supplied by the parent manager and verified against that remote head. Record this as `finalSHA`; neither `61af5455` nor another earlier candidate is implicitly final. Collect a fresh native Darwin artifact at exactly `finalSHA`.
5. In the matching current candidate checkout, run the Darwin rebaseline CLI with the final artifact's real IDs, `--source-sha <finalSHA>`, and `--reason cargo-producer-lock` plus the approved transition arguments below. This second stage must pass against the stage-1 contract, including unchanged version/pnpm, normalizer, fingerprint, fixtures, runners, tool/platform/source identities and all sixteen raw/normalized totals. Replay the strict budget checker after the re-pin, and retain both stages' evidence separately.

For stage 4, the existing native workflow command is a handoff instruction, not evidence that it ran:

```sh
gh workflow run island-size-darwin.yml --ref prototype/318-rolldown-native \
  -f ref=<finalSHA-read-back-from-remote> -f platform=darwin-arm64 -f enforce=false
```

Do not substitute the review source, an older green run, a Linux replay, or a graph digest for that final native source. Darwin requires native macos-15 arm64; Linux requires native Linux x64. Both require Rust 1.99.0, Node 24.14.0, zlib 1.3.1-e00f703, pnpm 12.8.2, and esbuild 0.25.12. Record the actual source/run/job/artifact IDs and digests, tool/platform identity, original checker result, both complete eight-fixture modes, both saved full-dist passes, package tarballs, and normalizer sidecars. Collection with `enforce=false` does not mean the original checker passed. If any raw or normalized total differs, this transition cannot repin it.

Linux needs only the final Cargo transition because its live version/pnpm identities already match. A qualifying native Linux Health artifact may supply that evidence without manual dispatch. Preserve its actual measured PR merge source separately from the PR head: they need not be the same SHA, and the Linux rebaseline records both. Verify the artifact belongs to the intended candidate and validate its checkout identities and saved bytes; do not relabel its merge source as `finalSHA`. This session's lack of a manual-dispatch tool does not authorize fabricating a run or treating a local measurement as a CI artifact. Neither Linux collection nor Linux replay completes either Darwin stage.

Only after qualifying native evidence exists may the corresponding existing platform rebaseline script use `--reason cargo-producer-lock` with its normal artifact/source arguments and all four ordinary Cargo arguments, plus:

```text
--expected-old-cargo-sha 2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e
--expected-new-cargo-sha 13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37
--old-cargo-ref 2c988535e7c4dca5053941e430e390290c6b9396:research/v3-island-size/cargo-transitions/rolldown-native-318.old.lock
--reviewed-cargo-transition research/v3-island-size/cargo-transitions/rolldown-native-318.json
--added-packages <all 111 manifest delta.added identities, comma-separated>
```

Derive the allowlist from the reviewed inventory rather than copying it by hand:

```sh
node -e 'console.log(require("./research/v3-island-size/cargo-transitions/rolldown-native-318.json").delta.added.map(row => row.id).join(","))'
```

`--added-packages` lists new identities only for this reviewed mode; changed and removed records are bound separately by the exact manifest. The default invocation remains additive-only. The rebaseline scripts still recompute saved bytes, require unchanged raw and gzipNormalized totals and fixture set, match source/tool/platform/dependency identities, and allow only the existing Cargo hash/evidence keys to move. Neither ceilings nor allowances may move. Each platform remains independently pending until its own native evidence passes. No Linux-only command in this task repins Darwin.

The commands above now use the immutable `--old-cargo-ref` input from #4219. The original native runs used the tracked file as recorded in their historical contracts; those receipts are unchanged. `--old-cargo-lock` is optional byte-for-byte corroboration, and cannot replace the immutable source reference.

## Exact rustls security replacement (#4314)

[RUSTLS-4314.md](RUSTLS-4314.md) records the separately approved rustls 0.23.45 → 0.23.46 lock pair. Its own exact-byte decision checker is selected through the same reviewed-transition entry point. It does not broaden this historical #318 authorization or apply either live platform contract.

## Exact noyalib 0.0.58 adoption (#4268)

[NOYALIB-4268.md](NOYALIB-4268.md) records the owner-approved noyalib / noyalib-serde-yaml 0.0.55 → 0.0.58 lock pair. It uses the same exact-byte checker and reviewed-transition entry point as the #4314 replacements, and does not broaden this historical #318 authorization.

## Rolldown oxc edge (#4429)

[ROLLDOWN-OXC-4429.md](ROLLDOWN-OXC-4429.md) records the owner-approved edge-only move in which `zfb-rolldown-prototype` gains an edge to the already-locked `oxc@0.153.0`. It is a schema 2 `cargo-internal-rewire` declaration, not a reviewed producer-lock transition, and does not broaden this historical #318 authorization.

## Exact tokio security replacement (#4569)

[TOKIO-4569.md](TOKIO-4569.md) records the tokio 1.52.3 → 1.54.1 lock pair for RUSTSEC-2026-0386, approved under POLICY-02. It uses the same exact-byte checker and reviewed-transition entry point as the #4314 replacements, and does not broaden this historical #318 authorization or apply either live platform contract.
