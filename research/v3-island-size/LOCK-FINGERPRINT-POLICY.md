# Island-size lock fingerprint policy (proposal, pending owner review)

Status: **proposal** for [#4187](https://github.com/Takazudo/zudo-front-builder/issues/4187), written under sub-issue [#4189](https://github.com/Takazudo/zudo-front-builder/issues/4189) of epic [#4188](https://github.com/Takazudo/zudo-front-builder/issues/4188). Sub-issue [#4190](https://github.com/Takazudo/zudo-front-builder/issues/4190) implements it as `research/v3-island-size/lock-fingerprint.mjs`. Owner review is required before live activation (the merge into `main`). Nothing in this document claims owner approval.

Policy identity: `policyVersion: 1`, `parser: "yaml@2.9.0"`. Any change to the selection, classification, normalization, or canonical form below is a `policyVersion` bump, and the helper's own bytes are pinned by `helperSha256`.

## 0. Allowed movement of `dependencyFingerprint` by re-pin reason

Decided under [#4210](https://github.com/Takazudo/zudo-front-builder/issues/4210) (S2 of epic [#4208](https://github.com/Takazudo/zudo-front-builder/issues/4208)) for [#4198](https://github.com/Takazudo/zudo-front-builder/issues/4198) and item 9. `helperSha256` is appended to the result **after** `sha256` is computed over the canonical input (`lock-fingerprint.mjs`, `computeLockFingerprint`), and run provenance carries only `{policyVersion, sha256}` (`check-budget.mjs`, `provenanceFingerprintKeys`), so a helper-only edit cannot move a measured byte or any run's provenance. Each reason may move exactly the keys in its row; every other key, every total, and every identity input is refused fail-closed, and no reason has a force or ignore-provenance flag.

| Reason | May move in `dependencyFingerprint` | Native evidence | Status |
| --- | --- | --- | --- |
| `fingerprint-helper` | `helperSha256` only | none (T1-only; proves old blob at `--base-ref` = pinned value, and old vs new helper give identical `sha256` + `--explain` inventory on the real lock) | implemented by S4 [#4213](https://github.com/Takazudo/zudo-front-builder/issues/4213) |
| `dependency-graph` | `sha256` only | fresh, both platforms, totals unchanged | deferred: [#4218](https://github.com/Takazudo/zudo-front-builder/issues/4218) (item 9a) |
| `fingerprint-policy` | `policyVersion` + `parser` + `sha256` + `helperSha256`, only together with a `policyVersion` increment | fresh, both platforms, totals unchanged | deferred: [#4218](https://github.com/Takazudo/zudo-front-builder/issues/4218) |
| `release-bump`, `runtime-identity`, `lockfile-provenance`, `fingerprint-migration` | none (all four keys refused) | as today | unchanged |

A helper edit that changes the fingerprint's value is by definition a policy change, not helper maintenance: `fingerprint-helper` refuses it, and the path is `fingerprint-policy`. The rejected alternative for #4198 was a documented full re-pin; no existing reason accepts `helperSha256` movement, so it would still need either a hand edit (bypassing every fail-closed check) or a new reason anyway.

Audited at `base/4187-island-lock-fingerprint` (head `183402a4`), `pnpm-lock.yaml` 6301 lines, package version 4.4.0. Every count and key list below was produced by parsing **both** YAML documents with `yaml@2.9.0` `parseAllDocuments(text, {uniqueKeys: true})` (0 errors, 0 warnings in each document) and walking the result, not by reading the file by eye.

## What the measured bytes are made of

The island-size contracts measure JavaScript emitted by the real `zfb build` against consumers whose `node_modules/@takazudo/{zfb,zfb-runtime,zfb-slugify}` point at workspace or packed packages and whose `node_modules/hono` points at the lock-installed Hono. Producing inputs, and where each one is pinned:

| Producing input | Comes from | Pinned by |
| --- | --- | --- |
| `zfb` CLI binary | Rust workspace | `toolchain.cargoLockSha256` (`Cargo.lock`, unchanged by this policy) |
| esbuild 0.25.12 | Rust-staged `crates/zfb/binaries/esbuild/esbuild`, verified by the SHA-256 constants in `crates/zfb/build.rs` | `toolchain.esbuildVersion` + run provenance `esbuildBinarySha256`; **not** from `pnpm-lock.yaml` |
| `dist/` of the three packages | `tsc` (`"build": "tsc"` in all three) → `typescript@7.0.2` from each package importer's `devDependencies` | this fingerprint (package importers) + `buildInputs` (item 8b) |
| Packed tarballs | `pnpm pack` → pnpm 12.8.2 | this fingerprint (environment document) |
| Hono 4.12.25 | `packages/zfb-runtime` `dependencies.hono` (`>=4.12.25` → `4.12.25`, override in `pnpm-workspace.yaml`) | this fingerprint + `toolchain.honoVersion` |
| Measurement runners | `measure-real.mjs`, `measure.mjs` | `runnerSha256` |
| Fixtures | `fixtures/*.tsx` | `fixtureSha256` |
| Normalizer | `normalize-identity.mjs` | `normalizerSha256` |
| Node + zlib | runner host | `toolchain.nodeVersion`, `toolchain.zlibVersion` |

The `esbuild@0.28.1` and `esbuild@0.27.7` snapshots in the lock are Vite+ peer resolutions (test tooling), not the measurement esbuild.

## 1. Importer table (project document)

Selected roots: `packages/zfb`, `packages/zfb-runtime`, `packages/zfb-slugify`, plus every importer reached recursively through a `link:` edge. Each selected importer contributes its `dependencies`, `devDependencies`, and `optionalDependencies`. The lock lists no importer `peerDependencies` section (lockfile v9 importers carry only those three sections; `packages/zfb-runtime/package.json`'s `peerDependencies` on `@takazudo/zfb` is covered by its `devDependencies` `link:../zfb` edge), so there is nothing else to cover.

**No fine-grained exclusion of the packages' test dependencies this round.** `vite-plus`, `happy-dom`, `@types/node`, and the `typescript-5.9` / `typescript-6.0` aliases are **included**, conservatively. They do not produce measured bytes, but splitting a package importer by purpose is deferred to an explicit owner choice (item 9b).

| Importer | Verdict | Reason |
| --- | --- | --- |
| `.` (project document) | **Excluded** | Root contributor tooling; no entry produces measured bytes (audit in item 2). |
| `crates/zfb-md-wasm/npm` | Excluded | `@takazudo/zfb-md-wasm` is not part of the island bundles; not linked from any selected importer. |
| `docs` | Excluded | Docs site. `linkWorkspacePackages: false`, so it consumes registry `@takazudo/zfb@4.4.0` / `@takazudo/zfb-runtime@4.4.0(@takazudo/zfb@4.4.0)`, not `link:` edges. |
| `packages/create-zfb` | Excluded | Generator CLI. It links **to** `packages/zfb` (`workspace:4.4.0` → `link:../zfb`); nothing selected links to it. |
| `packages/zfb` | **Included (root)** | Measured SDK; `tsc` build. |
| `packages/zfb-adapter-cloudflare` | Excluded | Cloudflare adapter; not used by the island measurement and not linked from any selected importer. |
| `packages/zfb-darwin-arm64` | **Included (via link)** | `packages/zfb` `optionalDependencies` → `link:../zfb-darwin-arm64`; importer entry is `{}`. |
| `packages/zfb-darwin-x64` | **Included (via link)** | Same; `{}`. |
| `packages/zfb-linux-arm64-gnu` | **Included (via link)** | Same; `{}`. |
| `packages/zfb-linux-x64-gnu` | **Included (via link)** | Same; `{}`. |
| `packages/zfb-runtime` | **Included (root)** | Measured runtime; `tsc` build; brings Hono. Its `devDependencies` `link:../zfb` (`workspace:*`) points at an already-selected root. |
| `packages/zfb-slugify` | **Included (root, also via link)** | Measured helper; `tsc` build. Also reached from `packages/zfb` `dependencies` → `link:../zfb-slugify`. |
| `packages/zfb-win32-x64-msvc` | **Included (via link)** | Same as the other platform importers; `{}`. |

Thirteen importers in the project document, each listed once. `examples/*` is a workspace glob but currently contains no package, so it has no importer.

Reach from the selected importers at the audited head: **8 importers, 230 of 555 snapshots, 225 of 538 package records**, 0 missing. The reached set includes `typescript@7.0.2`, `typescript@6.0.3`, `typescript@5.9.3`, `hono@4.12.25`, and `yaml@2.9.0` (as a Vite+ peer).

## 2. Root producing-tool audit (`.` importer, project document)

The root `.` importer has **no** `typescript`. Its `devDependencies` today:

| Entry | Locked | Why it does not produce measured bytes |
| --- | --- | --- |
| `@playwright/test` | `^1.49.0` → 1.61.0 | Browser test runner (router-chromium, wind, zudo-react lanes); never runs in the build/pack/measure path. |
| `@takazudo/mdx-formatter` | 1.3.0 | Markdown formatter (`format:*:mdx`); touches no package source or output. |
| `html-validate` | 10.17.0 → `10.17.0(vitest@5.0.1)` | Validates emitted HTML in the showcase jobs; the island guard measures JS only. |
| `lefthook` | `^2.1.6` → 2.1.6 | Git hooks (`prepare`); not invoked by builds. |
| `vite-plus` | `catalog:` → `1.0.0(…)(typescript@6.0.3)(yaml@2.9.0)` | Root test runner (`vp test`). The packages' own `vite-plus` is a separate snapshot `1.0.0(…)(typescript@7.0.2)(yaml@2.9.0)`, reached and included through the package importers. |
| `wrangler` | 4.85.0 | Deploy CLI for docs/showcase. |
| `ws` | `^8.18.0` → 8.18.0 | WebSocket client for test scripts. |

`measure-real.mjs`, `measure.mjs`, `check-budget.mjs`, and `normalize-identity.mjs` import only `node:` built-ins and sibling files; the CI build/pack steps call `pnpm -C packages/<pkg> build|pack`, which resolves `tsc` from the package importer, not from the root.

**Verdict:** the project-document root importer is **excluded**. The **environment document** (document 1: `importers['.'].configDependencies`, `importers['.'].packageManagerDependencies` with pnpm 12.8.2, its 15 `packages` and 15 `snapshots` for `pnpm` and the 14 `@pnpm/exe.*` binaries) is **included in full**, because pnpm itself packs the tarballs the packed mode measures. Adding a producing tool to the root later (for example a root-level bundler step in the build) requires a `policyVersion` bump; it is not picked up silently.

## 3. Field classification (both documents)

Verdicts: **included** (hashed), **deliberately excluded** (reason given), **rejected** (fail closed: the helper throws). Schema validation runs over **every** record in both documents, including unreached ones, so the parser's shape assumptions hold globally. A record that is reached is hashed **completely**; no field inside a reached package or snapshot record is dropped.

### Top-level keys

| Key | Doc 1 (environment) | Doc 2 (project) | Present today | Verdict |
| --- | --- | --- | --- | --- |
| `lockfileVersion` | yes | yes | both | Included; must be the string `'9.0'` in each document, else rejected. |
| `settings` | — | yes | doc 2 | Included in full (`autoInstallPeers`, `excludeLinksFromLockfile`). In doc 1 → rejected. |
| `catalogs` | — | yes | doc 2 | Included in full (global, conservative: today only `default.vite-plus`). In doc 1 → rejected. |
| `overrides` | — | yes | doc 2 | Included in full (`devalue`, `hono`, `vite@*`). In doc 1 → rejected. |
| `patchedDependencies` | — | allowed | absent | Included in full when present (patches change installed bytes). In doc 1 → rejected. |
| `pnpmfileChecksum` | — | allowed | absent | Included when present (a pnpmfile can rewrite resolution). In doc 1 → rejected. |
| `ignoredOptionalDependencies` | — | allowed | absent | Included in full when present (changes which optional deps install). In doc 1 → rejected. |
| `packageExtensionsChecksum` | — | allowed | absent | Included when present (package extensions rewrite manifests). In doc 1 → rejected. |
| `importers` | yes | yes | both | Doc 1: included in full. Doc 2: selected importers included (normalized per item 4); the other five deliberately excluded (item 1). |
| `packages` | yes | yes | both | Doc 1: included in full. Doc 2: records reached from selected snapshots included completely; unreached records deliberately excluded (not producing inputs). |
| `snapshots` | yes | yes | both | Doc 1: included in full. Doc 2: reached records included completely; unreached deliberately excluded. |
| any other key (e.g. `time`) | rejected | rejected | — | Rejected (unknown → policy bump). |

### Importer entries

| Field | Doc 1 `.` | Doc 2 importers | Verdict |
| --- | --- | --- | --- |
| `configDependencies` | yes (`{}`) | — | Included (environment, in full). In doc 2 → rejected. |
| `packageManagerDependencies` | yes (`pnpm` 12.8.2) | — | Included (environment, in full). In doc 2 → rejected. |
| `dependencies` | — | yes | Included for selected importers; excluded with the importer otherwise. In doc 1 → rejected. |
| `devDependencies` | — | yes | Included for selected importers (test deps included this round). In doc 1 → rejected. |
| `optionalDependencies` | — | yes | Included for selected importers (and followed for `link:` edges). In doc 1 → rejected. |
| `peerDependencies`, `dependenciesMeta`, `publishDirectory`, any other | — | absent | Rejected (none appears today; a new section needs a policy decision). |

Doc 1 has exactly one importer, `.`; any other doc-1 importer is rejected.

### Dependency-edge records (inside an importer section)

| Field | Verdict |
| --- | --- |
| `specifier` | Included (string). Only the lockstep rule of item 4 rewrites it. |
| `version` | Included (string). `link:<rel>` → importer edge (item 5); otherwise a snapshot reference. |
| any other / missing either field | Rejected (malformed edge). |

### Package records (`packages`)

| Field | Doc 1 | Doc 2 | Verdict |
| --- | --- | --- | --- |
| `resolution` | yes | yes | Included. Only `{integrity}` is accepted; `tarball`, `type`, `directory`, `commit`, `repo`, `path` or any other sub-key → rejected (non-registry resolution needs a policy decision). |
| `engines` | yes | yes | Included. |
| `cpu` | yes | yes | Included (sequence order preserved). |
| `os` | yes | yes | Included. |
| `libc` | yes | yes | Included. |
| `hasBin` | yes | yes | Included. |
| `peerDependencies` | — | yes | Included. |
| `peerDependenciesMeta` | — | yes | Included. |
| `deprecated` | — | yes | Included (reached records are hashed completely, even though it carries no bytes). |
| any other (`bundledDependencies`, `name`, `version`, `requiresBuild`, …) | rejected | rejected | Rejected. |

### Snapshot records (`snapshots`)

| Field | Doc 1 | Doc 2 | Verdict |
| --- | --- | --- | --- |
| `dependencies` | — | yes | Included; each value is a snapshot edge (followed). |
| `optionalDependencies` | yes | yes | Included; followed. |
| `optional` | yes | yes | Included. |
| `transitivePeerDependencies` | — | yes | Included. |
| any other (`id`, `patched`, `bundledDependencies`, …) | rejected | rejected | Rejected. |

Completeness check: the parsed key sets in the real lock are exactly doc 1 top `{lockfileVersion, importers, packages, snapshots}`, doc 2 top `{lockfileVersion, settings, catalogs, overrides, importers, packages, snapshots}`, importer entries `{configDependencies, packageManagerDependencies}` / `{dependencies, devDependencies, optionalDependencies}`, edge fields `{specifier, version}`, package fields `{resolution, engines, peerDependencies, peerDependenciesMeta, cpu, os, libc, hasBin, deprecated}`, `resolution` sub-keys `{integrity}`, snapshot fields `{dependencies, optionalDependencies, optional, transitivePeerDependencies}`. Every one is classified above. Scalar leaves are only strings (3166) and booleans (357); there are no numbers or nulls.

## 4. Lockstep normalization rule

Exactly one rewrite: within **selected** importers, a `specifier` whose value is exactly `workspace:<V>`, where `<V>` is the current `packages/zfb/package.json` `version`, becomes `workspace:<lockstep>`. Today that is the six `workspace:4.4.0` specifiers in `packages/zfb` (one `dependencies`, five `optionalDependencies`).

Untouched: `workspace:*` (`packages/zfb-runtime` → `@takazudo/zfb`), any other `workspace:` form (`workspace:^…`, `workspace:~…`, a different version), every `version` value including `link:` targets, and every package/snapshot record. Nothing else version-like is normalized; registry versions of `@takazudo/*` in excluded importers are not visited at all.

## 5. Link resolution rule

A `version` beginning with `link:` is an importer edge. Resolve the remainder with POSIX semantics relative to the **owning importer's** path: `target = posix.normalize(posix.join(owner, rel))`. Then:

- Absolute `rel`, or a `target` equal to `..` or starting with `../` → **escape** error (the link leaves the workspace). From `packages/zfb`, `link:../../../outside` normalizes to `../outside`.
- Otherwise, if `target` is not a key of the project document's `importers` → **missing importer** error. From `packages/zfb`, `link:../../outside` normalizes to `outside`, which is inside the repo root but not an importer.
- A `target` of `.` (the excluded root importer) → rejected: following it would silently pull the root back in; it needs a policy bump.
- An empty remainder (`link:`) → malformed edge.

These are two distinct error messages (escape vs missing importer), so a test can tell them apart. `link:` values are accepted only in importer edges; a `link:` or `file:` value in a snapshot edge is a malformed edge. Importers are walked with a visited set: `packages/zfb-runtime` → `packages/zfb` → `packages/zfb-slugify` reaches `packages/zfb-slugify` a second time as a root, and a future two-way link must not loop.

Snapshot edge resolution, for completeness: a value whose part before the first `(` contains `@` after index 0 is an alias and is itself the snapshot key (`typescript-5.9` → `typescript@5.9.3`; `vite` → `@voidzero-dev/vite-plus-core@1.0.0(…)`); otherwise the key is `<name>@<value>`. Keys carry their full peer suffix: the root's `vite-plus@1.0.0(…)(typescript@6.0.3)(yaml@2.9.0)` and the packages' `vite-plus@1.0.0(…)(typescript@7.0.2)(yaml@2.9.0)` are different snapshots, and only the latter is reached. The package record key is the snapshot key up to its first `(`.

## 6. Fail-closed list (#4190 must implement each)

1. Duplicate keys in either document (`uniqueKeys: true` error).
2. Not exactly two YAML documents; document 1 lacking `importers['.'].packageManagerDependencies`, or document 2 lacking `settings`/`importers`.
3. `lockfileVersion` ≠ the string `'9.0'` in either document.
4. A selected root importer missing from the project document.
5. A `link:` escape, a `link:` to a missing importer, or a `link:` to `.` (item 5).
6. A snapshot referenced (from an importer or snapshot edge) but absent.
7. A package record absent for a reached snapshot.
8. Unknown top-level, importer-entry, edge, package-record, `resolution`, or snapshot-record keys (item 3), in either document.
9. Unsupported YAML node kinds: explicit tags, anchors or aliases (including merge keys `<<`), non-scalar keys, non-string keys; scalar leaves other than strings and booleans.
10. Malformed edge values: non-string, empty, unbalanced peer-suffix parentheses, an edge record without exactly `specifier` + `version`, `link:`/`file:` in a snapshot edge.
11. Any parser `errors` or `warnings` on either document.

Cycles in the importer graph and the snapshot graph are handled with visited sets; they are not errors.

## 7. Parser decision

Add `yaml` as a **root devDependency, exact `2.9.0`** (it is already resolved transitively, as a Vite+ peer, so no new package bytes are fetched; `yaml@2.9.0` and `js-yaml@4.3.2` exist only transitively today, so a root script cannot `import "yaml"` yet). Parse with `parseAllDocuments(text, {uniqueKeys: true})`, fail on any `doc.errors` or `doc.warnings`, and pin the identity as `parser: "yaml@2.9.0"` in the fingerprint record.

Expected side effect, for #4190 to verify rather than assume: adding the root devDependency changes only the excluded root importer (the `yaml@2.9.0` package and snapshot records already exist and are already reached), so the fingerprint should be unchanged by that lock edit, while the v2 whole-file `pnpmLockSha256` moves (the known expected-red interval recorded on epic #4188).

Canonical form, recommended for #4190: a JSON object with recursively sorted keys and sequence order preserved, serialized without whitespace and hashed with SHA-256 over its UTF-8 bytes. Top-level members: `policyVersion`, `parser`, `environment` (document 1 in full), `project` (`lockfileVersion`, `settings`, `catalogs`, `overrides`, any present optional global key from item 3, and the selected `importers`, reached `packages`, reached `snapshots`), and `buildInputs` (item 8b). The contract records `dependencyFingerprint {policyVersion, parser, helperSha256, sha256}`.

## 8. Guarantee wording

The fingerprint is **invariant to changes outside the included inputs**. Example: a docs-only registry bump that touches no shared reachable snapshot, catalog entry, override, or setting leaves it unchanged. A docs change that alters a **shared reachable snapshot** (say, re-resolving a transitive dependency that the packages also reach, or changing its peer suffix) legitimately moves the fingerprint; that is the guard working, not a false alarm. This policy does **not** promise that all docs or root bumps are invisible.

## 8b. Build-recipe pinning (decision)

#4187 Phase A requires that "the root-selection policy and relevant build commands/settings must themselves be pinned". **Decision: adopt `buildInputs`.** The canonical input gains a `buildInputs` section:

- Per selected importer (all eight, keyed by importer path): the `package.json` strings `scripts.build`, `scripts.prepack`, `scripts.prepare`, `scripts.postpack`, each `null` when absent, and `tsconfigSha256` = SHA-256 of that importer's `tsconfig.json` bytes, `null` when absent. Today: `tsc` for the three measured packages, no pack lifecycle scripts anywhere, and `null` everything for the five platform importers. The pack lifecycle scripts are included because `pnpm pack` can run them while producing the packed-mode tarballs (`prepare` is listed conservatively).
- Shared: `tsconfigBaseSha256` = SHA-256 of the root `tsconfig.base.json`, which all three package `tsconfig.json` files extend.

The selection policy itself is pinned by `policyVersion` + `helperSha256`. Already pinned elsewhere and not duplicated: `measure-real.mjs` / `measure.mjs` (`runnerSha256`), fixtures (`fixtureSha256`), the normalizer (`normalizerSha256`), `Cargo.lock` (`cargoLockSha256`), and the Node/zlib/esbuild/Hono versions (`toolchain`).

Explicit boundaries (**not** pinned by this fingerprint):

- The CI build and pack commands live in the workflows (`.github/workflows/health.yml` "Build and pack packages for island-size measurement", line 912, and `.github/workflows/island-size-darwin.yml` "Build and pack SDK and runtime", line 132): `pnpm -C packages/<pkg> build` and `pnpm -C packages/<pkg> pack`. Their text is reviewed with workflow changes, not hashed.
- `package.json` fields other than the four scripts (`files`, `exports`, `publishConfig`, …) are product shape: a change there shows up in the freshly measured bytes and is judged by the ceilings.
- The `packages/zfb` test-only `tsconfig.zudo-react-*.json` files are not part of `build`.
- `pnpm-workspace.yaml` is pinned only through its projections into document 2 (`settings`, `catalogs`, `overrides`); its install-policy keys (`allowBuilds`, `minimumReleaseAge`, `engineStrict`, `peerDependencyRules`) do not change the resolved graph. `linkWorkspacePackages` does, but its effect is visible as `link:` versus registry `version` values in the importers.

This epic edits no `health.yml` test wiring (per `crates/CLAUDE.md`); this topic changes no file other than this document.

## 8c. Parser-bytes boundary

The parser is pinned by the identity string `yaml@2.9.0` only. The installed `yaml` package bytes and integrity are **not** part of the fingerprint, because they live under the excluded root importer. This is accepted: the fingerprint is computed over canonical semantic content, so the parser's bytes do not change its value. A parser bug would surface as a parse error or a different canonical value, both of which fail closed, and replacing the parser is a `parser` identity change, which is an explicit policy bump. The helper refuses to run when the installed `yaml` package version differs from the pinned `parser` identity, so a root `yaml` bump cannot change the parser behind an unchanged identity string.

## 9. Owner choices (recorded under #4210, implementation deferred)

- **(a) Post-v3 rebaseline reason for a reachable dependency change.** After migration, a change inside the included inputs (for example a TypeScript bump) moves the fingerprint and needs a native re-pin. `fingerprint-migration` covers only the one-time 2 → 3 migration. **Recorded recommendation:** a narrow `dependency-graph` reason that may move `sha256` only, requires fresh native evidence on both platforms, and refuses any total, identity, `policyVersion`, `parser`, or `helperSha256` movement (table in item 0). **Deferred** to [#4218](https://github.com/Takazudo/zudo-front-builder/issues/4218); until it lands, a reachable dependency change has no re-pin path and is a reviewed decision.
- **(b) Excluding the packages' test-only dependencies.** This round includes `vite-plus`, `happy-dom`, `@types/node`, and the `typescript-5.9` / `typescript-6.0` aliases. **Decision: keep them included; deferred.** Splitting package importers by purpose (and how to prove a dependency is test-only) would be a `policyVersion` bump and is not scheduled.
- **(c) Policy-version transitions.** A change to the selection, classification, normalization, canonical form, or parser identity is a `policyVersion` bump; no reason accepts it today. **Recorded:** a `fingerprint-policy` reason that may move `policyVersion` + `parser` + `sha256` + `helperSha256` only together with a `policyVersion` increment and fresh native evidence on both platforms. **Deferred** to [#4218](https://github.com/Takazudo/zudo-front-builder/issues/4218).

## 10. Owner review

The importer table (item 1) and the field classification (item 3) are posted to #4187 as "Policy proposal (pending owner review)". No #4187 checklist item is ticked by this topic.
