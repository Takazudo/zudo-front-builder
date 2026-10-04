# Toolchain modernization: stage 1 pnpm 12 evidence (#3555)

Compared with the stage 0 ledger ([`BASELINE.md`](./BASELINE.md), `baseline.json`). Local host and
load are as recorded there (macOS 26.6.1 arm64, 8 GiB, Node v24.14.0, shared and loaded). Every
command below ran in the foreground with stdin closed (`</dev/null`) and `CI` unset unless noted.

## Selected pnpm 12.8.2, not 12.9.x

| Version | Published (UTC)  | Age at selection | Reason                                                                                                                                                                                                                       |
| ------- | ---------------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 12.8.1  | 2026-09-28 17:38 | 5.6 days         | The #3544 planning candidate. Superseded by its own patch                                                                                                                                                                  |
| 12.8.2  | 2026-09-30 01:15 | 4.3 days         | **Selected.** Final patch of the 12.8 line. It fixes `PATH`/`Path` selection for lifecycle scripts, `pnpm run` SIGTERM exit as PID 1 and frozen-install CPU use. No newer 12.8.x exists                               |
| 12.9.0  | 2026-10-02 22:27 | 1.4 days         | A minor: it records every installed project in the store and bundled a WebAssembly build that grew the package to about 55 MB                                                                                             |
| 12.9.1  | 2026-10-03 20:48 | 12.4 hours       | Moved that WebAssembly build out again a day later. Too new to treat as validated                                                                                                                                          |

Selection was checked against the npm registry on 2026-10-04 09:14Z. `latest` and `latest-12` both
point at 12.9.1. A later stage can move to 12.9.x once it has aged; this stage gives no reason to.

pnpm 12 requires `pnpm/action-setup` **v6.1.0** ("feat: support pnpm v12", pnpm/action-setup#288).
All 24 call sites moved from `0e279bb9…` (v6.0.8) to commit
`ea17c68df8912ef543352723c149a84f56e3d413` (the commit the annotated `v6.1.0` tag points to).
`reusable-smoke-clean-room.yml` keeps its explicit `version:` input, now `12.8.2`, which matches
`packageManager`.

## Lockfile changed in two reviewed ways; installed package set unchanged

| Item                       | Baseline (pnpm 11.3.0)                                             | Stage 1 (pnpm 12.8.2)                                              |
| -------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------ |
| `pnpm-lock.yaml` SHA-256   | `919bc08a976ef1bc03bf6bb465dff020bff8b0c91c86d8eca04ebde4c192a0be` | `c8b344b9cb340f5185412a53300cfc3444f3a8569794db24b16c1472ac8f852b` |
| `lockfileVersion`          | `9.0`                                                              | `9.0` (both documents)                                             |
| `.pnpm` virtual-store dirs | 271                                                                | 271, the same `name@version` multiset                              |
| Hono embed tree            | 563 files, `ec39ef6b…` (see below)                                 | identical                                                          |

1. **Env-lockfile document.** A `--frozen-lockfile` install fails with
   `ERR_PNPM_FROZEN_LOCKFILE_WITH_OUTDATED_LOCKFILE` ("resolve package manager dependencies") until
   the lockfile records the pinned package manager. pnpm 12 prepends a YAML document whose root
   importer holds `packageManagerDependencies: pnpm 12.8.2`, plus `pnpm@12.8.2` and its 14
   `@pnpm/exe.*` optional platform packages, each with an integrity hash. The dependency document
   after it was byte-identical when only this document was added.
2. **Optional `supports-color` peer.** pnpm 12's first re-resolving install links the optional peer
   that `debug@4.4.3` declares to `supports-color@10.2.2`, which the graph already held. That
   suffixes 22 snapshot keys with `(supports-color@10.2.2)`: debug, micromark, mdast/remark,
   vitest, vite-node, html-validate, zudo-doc and `@takazudo/zfb-md-wasm@2.20.2`, all
   contributor-only. No version changes and no package is added or removed. The same version-bump
   cycle under pnpm 11.3.0 left the lockfile byte-identical, so this is pnpm 12 behavior.

The re-resolution was committed in this stage on purpose. Otherwise the first release version bump
would carry this peer churn. `check-budget.mjs` normalizes only `workspace:<version>` specifiers, so
the island-size lock hash would then fail after the release commit. With the re-resolved lockfile,
a repeat `pnpm install` and `pnpm install --frozen-lockfile` both left it byte-identical.

Both island-size contracts record the new SHA-256 (`toolchain.pnpmLockSha256`). The README note
states the size totals are unchanged, and the Linux health size step must prove that.

## Native embedding gets byte-identical Hono 4.12.25

`crates/zfb/build.rs::embed_framework_packages` needs no change. A Node snapshot script (scratch,
not committed) reimplemented `locate_pnpm_pkg` and the `copy_pkg_published` filter: skip
`node_modules`, dotfiles, `__tests__`/`test`/`tests`, `*.map` and non-regular entries. It hashed
every remaining file.

| Install                                  | Store dir                                         | Real directory, not a symlink | Files | Tree SHA-256                                                       |
| ---------------------------------------- | ------------------------------------------------- | ----------------------------- | ----: | ------------------------------------------------------------------ |
| Clean `git archive`, pnpm 11.3.0, frozen | `node_modules/.pnpm/hono@4.12.25/node_modules/hono` | yes                           |   563 | `ec39ef6b3c0c549580a703ba2969e5da7f77caa41be0cd744f58af4699815bb3` |
| Clean `git archive`, pnpm 12.8.2, frozen | same                                              | yes                           |   563 | same                                                               |
| Clean `git worktree` at the stage commit | same                                              | yes                           |   563 | same                                                               |

pnpm 12 keeps `nodeLinker: isolated`, `virtualStoreDir: .pnpm`, `layoutVersion: 5` and the
`store/v11` content-addressable store. Its `enableGlobalVirtualStore` is unset, so store entries
stay real directories. The Cargo build itself was not run on this host. CI's release, health,
no-v8 and binary-smoke jobs compile `build.rs` against this layout.

## Installation matrix passed

| Check                                                     | Command                                                                     | Result                                                                                                                       |
| --------------------------------------------------------- | --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Fresh frozen install, clean tree                          | `pnpm install --frozen-lockfile` in a `git archive` copy                    | pass, 271 packages, 22.8 s cold for pnpm 12 (pnpm 11: 14.9 s, without rerunning the esbuild and workerd postinstalls) |
| Clean worktree setup                                      | `git worktree add --detach` at the stage commit, then a frozen install      | pass, 5.5 s warm. `prepare` ran (lefthook and `install-git-hooks.sh`)                                                        |
| Repeat frozen / repeat plain install                      | both, twice                                                                 | "Lockfile is up to date" / "Already up to date". No lockfile churn                                                           |
| Upgrade an existing pnpm 11 tree in place                 | pnpm 12 `install` over a pnpm-11 `node_modules`                             | "Already up to date". No purge was needed                                                                                    |
| Release-style version bump                                | bump all workspace packages to 3.1.1, then `pnpm install`                   | pass. Only version lines changed in the re-resolved lockfile                                                                 |
| Forced modules purge                                      | `install --frozen-lockfile --public-hoist-pattern='*lodash*'`, then revert  | pass, exit 0, purged and relinked with no prompt both ways. The restored tree matched the snapshot                           |
| Root filter (showcase jobs)                               | `pnpm install --frozen-lockfile --filter zudo-front-builder`                | pass. Root only; `html-validate` and `wrangler` bins present                                                                 |
| Member filter (`docs:install`)                            | `pnpm --filter docs install --frozen-lockfile`                              | pass. The docs host still links registry `@takazudo/zfb@2.20.2`, not the workspace                                           |
| Workspace isolation                                       | link targets                                                                | `workspace:` deps link locally (`zfb-runtime` → `../../../zfb`); `linkWorkspacePackages: false` kept                         |
| Native optional packages                                  | `.pnpm` entry set                                                           | unchanged, including `@esbuild/*`, `@img/*` and workerd platform packages                                                    |
| Allowed builds                                            | install log                                                                 | only the four `allowBuilds` entries ran scripts: esbuild ×4 versions, lefthook, sharp, workerd                              |
| Lifecycle hooks                                           | `git commit` in the worktree                                                | lefthook pre-commit (`format-mdx`, `format-ts`) ran under pnpm 12.8.2                                                       |
| Cargo integration                                         | `pnpm config get cargo.enabled`, `git status Cargo.lock`                    | unset (off); `Cargo.lock` untouched                                                                                          |
| Auto-switch from an installed pnpm 11                     | `pnpm --version` in the worktree with global pnpm 11.3.0                    | `12.8.2`                                                                                                                     |
| `actions/setup-node` `cache: pnpm`                        | `pnpm store path --silent`                                                  | `…/pnpm/store/v11`, exit 0                                                                                                   |

## `confirmModulesPurge` removed because pnpm 12 rejects it

pnpm 12.8.2 refuses the whole install with `ERR_PNPM_UNRECOGNIZED_WORKSPACE_SETTINGS`:
`"confirmModulesPurge" (a pnpm v11 setting)`. The forced-purge row above shows that pnpm 12 purges
and relinks under closed stdin, with `CI` unset, without the
`ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY` abort that #593 worked around. The setting and its
comment are deleted. `scripts/build-macos-x64-local.sh` keeps `CI=true` to match the runner.

Every other policy reads back unchanged through `pnpm config get`: `linkWorkspacePackages=false`,
`autoInstallPeers=true`, `engineStrict=true` and `minimumReleaseAge=0`. The `devalue` and `hono`
overrides are still recorded in the lockfile, and `allowBuilds` was not broadened.

## Changed CLI forms have no callers

A repository-wide search found no `--resolution-only`, and no `--frozen-lockfile false` or
`--frozen-lockfile=true|false` caller. The only explicit form is
`crates/zfb-md-wasm/npm/test/package-browser.test.ts`'s `--no-frozen-lockfile`, which pnpm 12
supports. `pnpm pack --pack-destination` still prints the absolute tarball path as its last stdout
line. `scripts/smoke-packed-clean-room.sh` and `release.yml`'s md-wasm pack step depend on that.

## Engine and peer strictness exposed nothing

Every `engines.node` range in the lockfile is satisfied by both CI Node 22.23.3 and local 24.14.0.
The `engineStrict` frozen installs, including the optional native subtrees, passed without
warnings. pnpm 12 also stopped printing pnpm 11's "Unsupported platform" warnings for the four
foreign `packages/zfb-*` platform workspace packages; they are still installed as workspace links.

## Package contracts are unchanged apart from tarball bytes

The same freshly built `dist/` was packed with both versions (`pnpm -C <pkg> pack`). pnpm 11.3.0
reproduced the stage 0 tgz sizes exactly (267,310 / 133,889 / 18,316 / 4,981 bytes).

| Tarball                                     | Files (pnpm 11 / 12) | Contents                                                                                                                       | tgz bytes (pnpm 12) | SHA-256 (pnpm 12)                                                  |
| ------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------: | ------------------------------------------------------------------ |
| `takazudo-zfb-3.1.0.tgz`                    | 120 / 120            | identical bytes and modes                                                                                                      |             268,495 | `84caa003cb24fcd98d1f7f989452adc1d3e73f62c7ee5ecdb4e9e62a37459127` |
| `takazudo-zfb-runtime-3.1.0.tgz`            | 46 / 46              | identical except `package.json` key order: pnpm 12 keeps the rewritten `@takazudo/zfb: 3.1.0` devDependency in place. Semantically equal | 134,600 | `4ffdaa22ed1846315bfb2cb858e8290915a0a6c6e6dc3608861a67774c0775f1` |
| `takazudo-zfb-adapter-cloudflare-3.1.0.tgz` | 15 / 15              | identical bytes and modes                                                                                                      |              18,532 | `c625729716b69671de815ae292b0140097928b969528c692b21f8292dcfb77de` |
| `create-zfb-3.1.0.tgz`                      | 7 / 7                | identical bytes and modes                                                                                                      |               4,981 | `ffd2c2273ad4a929c2b4c05b7374e46750b20ab53ddf04711c0d6e1dca779a3b` |

The larger tgz byte counts come from pnpm 12's gzip stream, not from contents. The island-size
contracts do not pin tarball digests; each run re-verifies the tarballs it packed.

An isolated consumer outside the repository (`packageManager: pnpm@12.8.2`, no workspace) installed
the three pnpm 12 tarballs with `--no-optional`. It resolved `hono@4.13.13` from `^4.12.25` and
imported all 20 non-JSON export subpaths: 14 of `@takazudo/zfb`, 4 of `@takazudo/zfb-runtime` and
2 of the adapter. The adapter's own `test:packed-types` packed-consumer check also passed inside
`pnpm test:workspace`.

## Workspace gates pass locally under pnpm 12.8.2

| Gate                       | Result                                                                                                                                                                       |
| -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm format:check`        | pass, 10.1 s                                                                                                                                                                 |
| `pnpm typecheck:workspace` | pass, 4.9 s (adapter + consumer, zfb, runtime)                                                                                                                               |
| `pnpm test:workspace`      | pass, 40.7 s: 7 projects, 101 files, **1,615** cases (baseline 1,614 plus one new wrangler-pin case). Root 23 files / 506, zfb 49 / 720, runtime 17 / 300, adapter 5 / 40, create-zfb 2 / 24, docs 5 / 25 |
| Compound steps             | the zfb `tsc -p` fixtures (zudo-react, dev, sdk) and the adapter `typecheck:consumer` + `test:packed-types` ran                                                               |
| Supervisor cleanup         | `docs-dev-supervisor` (6 cases, SIGINT/SIGTERM, exit status, descendant reaping) passed with pnpm 12 children. No `run-parallel`, history-server or vitest process remained |
| `tests/unit/*.sh`          | all 14 pass                                                                                                                                                                  |
| `pnpm audit --prod --audit-level=high` | exit 0 (1 low, 10 moderate). The output still carries the `vulnerabilities found` / `Severity:` finding markers `pnpm-audit-with-retry.sh` keys on                |

The first `test:workspace` run failed 2 cases in `showcase-workflow-tools.test.mjs`. They pin the
action-setup line; the test now expects v6.1.0.

## Two regressions found and fixed

- **`docs/scripts/check-wrangler-pin.mjs`** found the env document's root importer first, so it
  read `wrangler` as undefined and failed. It now searches only the last lockfile document. A new
  case in `scripts/__tests__/check-wrangler-pin.test.mjs` uses a two-document lockfile; it fails on
  the old parser and passes on the new one. This edits a gate-defining check and needs
  fresh-context review.
- **`showcase-workflow-tools.test.mjs`** pinned the old action-setup SHA. The assertion moved with
  the pin; its intent is unchanged.

## Classifier signature still matches pnpm 12

pnpm 12.8.2 still emits `ERR_PNPM_AUDIT_BAD_RESPONSE` (`pnpm/crates/cli/src/cli_args/audit/report.rs`
at `v12.8.2`). It now also uses that code for a reqwest transport failure, not only for bad status,
invalid JSON and unexpected bodies. `pnpm-audit-with-retry.sh` will therefore retry a network
failure that pnpm 11 reported differently. Findings still fail immediately, but reviewers should
know the infra class grew.

## Deliberately unchanged

- Consumer-facing pnpm 11 notes (README, `packages/create-zfb`, docs troubleshooting and
  first-site pages, `RELEASE_DAY_CHECKLIST.md`, release-skill trust-policy text) describe what
  users of published packages hit. Public consumer requirements are out of scope.
- Fixture user agents `pnpm/11.3.0` in `docs-dev-supervisor.test.mjs` and
  `supervisor-timeline-summary.test.mjs` are deterministic inputs, not pins. The first feeds
  `STEERING_ENV_KEYS`, so changing it would move the `env=` digest and cause a strict identity
  drift on `supervisor-watch`.
- `DEPENDENCIES.md` and `research/3242-*` record historical measurements taken with pnpm 11.3.0.

## CI must prove

- Every `pnpm/action-setup@v6.1.0` site bootstraps 12.8.2 on ubuntu and on the macOS legs that use
  it. `actions/setup-node` `cache: pnpm` restores against the changed lockfile key.
- Linux `build.rs` embedding, the no-v8 build, binary builds and Node-free smokes run on the pnpm 12
  tree. Scaffold E2E repeats the packed clean room with pnpm 12 `pnpm pack`.
- The health island-size step passes with the new `pnpmLockSha256` and unchanged Linux totals. If
  totals move, the manager rebaselines through the reviewed procedure. The Darwin contract's sizes
  stay with #3626.
- `pnpm audit (prod)`, docs-checks (`check:wrangler-pin` on the real lockfile), actionlint and the
  router/zudo-react/wind browser lanes (their path filters include `pnpm-lock.yaml`).
- `pnpm publish` of the non-platform packages runs only at release, so pnpm 12 publish and
  provenance remain unexercised until then. The `publish-npm-packages.sh` unit shim passed.

## Not run here

Cargo builds and tests, the md-wasm build and pack, browser lanes and docs build were not run, per
the 8 GiB host rule. Windows has no lane. Timings above were taken on a loaded shared host and are
not a speed claim.
