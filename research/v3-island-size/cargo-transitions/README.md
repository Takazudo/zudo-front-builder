# Reviewed Cargo producer transition for #318 / PR #4120

This decision authorizes one non-additive producer-lock proof. It does **not** admit current measurements, change numerical ceilings, repin either platform contract, or activate held #4234. The ordinary budget checker is unchanged. The root implementation manager owns final verification and native evidence reconciliation under [the development policy](../../../CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions).

The owner approved this narrow prerequisite after the whole-PR integration review at `adb507f7d3ed2de412c603f63cec6e38424dcadb`. The prior additive-only refusal remains valid history. The policy now recognizes only the exact pair below, with no general option to ignore changed dependencies.

| Input | Records | Whole-file SHA-256 |
| --- | ---: | --- |
| Historical main producer lock | 601 | `2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e` |
| Reviewed prototype producer lock | 696 | `13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37` |

[rolldown-native-318.json](rolldown-native-318.json) enumerates **111 new identities, 46 changed records, and 16 removed identities** with before/after record hashes. The old lock is preserved verbatim in [rolldown-native-318.old.lock](rolldown-native-318.old.lock), so policy tests do not depend on shallow checkout history. The implementation pins this exact pair, validates the complete record inventory, refuses duplicates/header/other-section changes, and pins the approved manifest byte digest `e4b89f02bea8ed6eace98023c08561ff9de2289a65d7fd5b14e0e3d6c1d008ed` in code. That digest binds the feature-graph identities and review rationale too; valid-shaped invented replacements and even unreviewed formatting changes are rejected. Future rebaseline evidence records the same digest. An unreviewed next lock is rejected even if an operator changes the artifact hash and command-line hash to agree with it.

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

**Both live contracts remain at the historical Cargo hash. Current-lock budget admission remains rejected.** The two live policy tests now validate historical synthetic fixture identities and explicitly require the three current-lock provenance failures after proving the reviewed structural transition. Their success means policy behavior is correct, not that current Linux or Mac shipped sizes have passed.

There is also a separate Darwin identity prerequisite: its live contract records package version **4.5.0** and pnpm lock `56e687fe05e0b84949881583381551b060cd357f43b0bf6b6bf0b27c2cc95b9a`, while this checkout and the Linux contract record **4.5.2** and `01972f66763ceaf7c0d526c43c0c0084de64f7cb5cea99b71ef9f79bf2789e0b`. The existing Cargo-only rebaseline rejects these differences. A fresh final-source Mac artifact is necessary but cannot by itself bypass that gate. Darwin needs a qualifying separate release-identity transition at the old Cargo lock, or a separately authorized composed decision. This lock-only approval grants neither, and no such re-pin was performed here.

No native Mac was started for this policy work. The final reviewed source SHA must be committed and pushed by the root manager before collecting evidence. The existing read-only workflow supports both native platform lanes:

```sh
gh workflow run island-size-darwin.yml --ref prototype/318-rolldown-native \
  -f ref=<final-reviewed-40-hex-SHA> -f platform=both -f enforce=false
```

Do not substitute the review source, an older green run, a Linux replay, or a graph digest for that final native source. Darwin requires native macos-15 arm64; Linux requires native Linux x64. Both require Rust 1.99.0, Node 24.14.0, zlib 1.3.1-e00f703, pnpm 12.8.2, and esbuild 0.25.12. Record the actual source/run/job/artifact IDs and digests, tool/platform identity, original checker result, both complete eight-fixture modes, both saved full-dist passes, package tarballs, and normalizer sidecars. Collection with `enforce=false` does not mean the original checker passed. If any raw or normalized total differs, this transition cannot repin it.

Only after qualifying native evidence exists may the corresponding existing platform rebaseline script use `--reason cargo-producer-lock` with its normal artifact/source arguments and all four ordinary Cargo arguments, plus:

```text
--expected-old-cargo-sha 2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e
--expected-new-cargo-sha 13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37
--old-cargo-lock research/v3-island-size/cargo-transitions/rolldown-native-318.old.lock
--reviewed-cargo-transition research/v3-island-size/cargo-transitions/rolldown-native-318.json
--added-packages <all 111 manifest delta.added identities, comma-separated>
```

Derive the allowlist from the reviewed inventory rather than copying it by hand:

```sh
node -e 'console.log(require("./research/v3-island-size/cargo-transitions/rolldown-native-318.json").delta.added.map(row => row.id).join(","))'
```

`--added-packages` lists new identities only for this reviewed mode; changed and removed records are bound separately by the exact manifest. The default invocation remains additive-only. The rebaseline scripts still recompute saved bytes, require unchanged raw and gzipNormalized totals and fixture set, match source/tool/platform/dependency identities, and allow only the existing Cargo hash/evidence keys to move. Neither ceilings nor allowances may move. Each platform remains independently pending until its own native evidence passes. No Linux-only command in this task repins Darwin.
