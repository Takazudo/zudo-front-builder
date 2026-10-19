# v4.5.4 release checkpoint (dogfood candidate, not a release)

Prepared 2026-10-19 by lane L5 under the owner's 20:38 UTC policy: prepare a checkpoint that
downstream projects can dogfood, without stopping implementation, and report the exact version,
the included changes and the remaining qualification before any actual release. This file is
that report. It does not approve, cut or publish anything: no tag, npm, Homebrew, GitHub Release
or deploy. The version (4.5.4, patch) is owner-approved; the release is not.

4.5.4 is a patch release of what is already on `main`. The Deno exit roadmap (direct V8,
issues 4301 → 4302 → 4303) is separate later work and is not part of it; see the last section.

## Candidate: `main` `013b0ab`

`013b0ab8fe5059e64ac0f7592a6eb938ac476e29` (merge of PR 4324, 2026-10-19 20:05 UTC). The four
owner-approved merges (4323 → 4328 → 4329 → 4324) landed in quick succession, so the push runs on
the three intermediate heads (`7f58a01`, `7426913`, `695ca66`) were cancelled or superseded.
`013b0ab` is the first head after them with a complete push-CI set.

| Workflow | Run | Result at `013b0ab` |
| --- | --- | --- |
| health (`health`, `build (no-v8)`) | [39316381257](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381257) | success (first attempt) |
| Binary smoke, scaffold E2E & showcase | [39316381226](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381226) | success |
| actionlint | [39316381210](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381210) | success |

This is hosted Linux evidence only. All three are green on the first attempt, so `013b0ab` is
the candidate. The last fully green `main` before it is `21ef7c4` (health
[39294618774](https://github.com/Takazudo/zudo-front-builder/actions/runs/39294618774)).

One external effect already happened on the way: merging PR 4329 pushed `docs/**` to `main`, so
[docs-deploy 39316341746](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341746)
deployed the docs site at `695ca66`. That deploy carried only the corrected from-source install
page; no v4.5.4 page exists under `docs/src/content/docs/changelog/` on `main`.

## Included changes since `v4.5.3`

`v4.5.3..013b0ab`, first-parent merges on `main`, oldest first. "Notes" says where each change
appears in the five-lane release notes ([drafts](changelog/)); everything marked "none" has no
package-facing effect.

| Merge | PR | Change | Notes |
| --- | --- | --- | --- |
| `d28cdd5` | 4234 | Island-size immutable provenance and bounded fingerprint capabilities (aggregation) | none (tooling) |
| `d1c8b28` | 4260 | Retain complete Linux size evidence after early contract failures | none (CI) |
| `e1ef4f5` | 4259 | Bounded cargo-internal-rewire re-pin reason (capability only) | none (tooling) |
| `a3999da` | 4295 | Re-pin the Darwin arm64 island-size contract to v4.5.3 | none (contract) |
| `f69c278` | 4242 | Direct V8 prototype behind `zfb-js-host`'s `direct-v8` feature, plus the behavior-preserving shared host-services refactor (`4a1bc10`) | none: the CLI never selects `direct-v8` |
| `fca8719` | 4290 | Direct-V8 harness canonicalizes a symlinked macOS TMPDIR | none (test) |
| `113a63e` | 4296 | Canonical importer expectations and retained failed retry reports | none (test) |
| `15a7621` | 4298 | Islands: shipped islands bytes stay stable across edits outside the browser graph | `zfb` Bug Fixes and Other Changes |
| `f6956d2` | 4297 | Docs organized around the zfb CLI workflow | none (docs) |
| `040bc5c` | 4299 | Bind island-size evidence to the selected platform | none (tooling) |
| `21d6807` | 4300 | Authenticate wind reference acquisition with the existing CI token | none (CI) |
| `502d9e6` | 4312 | Checkpoint: 4308 (symlink-alias graph paths), 4309 (bounded watcher and dev tests), 4313, 4316, 4318 | 4308 in `zfb` Bug Fixes; the rest none |
| `21ef7c4` | 4322 | 4310 external-edge admission capability, 4321 and 4320 capture fixes | none (tooling; 4310 is not lock admission) |
| `7f58a01` | 4323 | Roadmap ledger | none (docs) |
| `7426913` | 4328 | Store-installed and linked packed builds must emit byte-identical `dist/` (4231) | none (test) |
| `695ca66` | 4329 | Current-state README and toolchain docs; these release-prep drafts | none (docs) |
| `013b0ab` | 4324 | Pin cargo-nextest 0.9.140 in health and exam | none (CI) |

Package source check: `git diff v4.5.3 013b0ab -- packages/ crates/zfb-md-wasm` is empty, and
`Cargo.lock` changes only by the internal `zfb-js-host` crate (`publish = false`). The two
package-facing fixes are both in the `zfb` lane. The other four lanes are
`- No package-specific changes.`. The version evidence and breaking-change review are in
[README.md](README.md).

## Security lane: not on `main`, not in this checkpoint

| Item | Where it is | State |
| --- | --- | --- |
| PR 4315: rustls 0.23.45 → 0.23.46 (fixes the advisories in issue 4314) | `base/4314-security-darwin-evidence` `7e79458` (merge `9f30978`) | Integrated on that base only; `main`'s `Cargo.lock` still has rustls 0.23.45 |
| PR 4319: staged local Darwin evidence for the transition | same base (merge `2668020`) | Integrated on the base |
| PR 4326: one local Darwin evidence module (4311 + 4319) | same base (merge `7e79458`) | Integrated on the base |
| PR 4317: hosted Linux exact-source lane | draft, open on the base | Owner merge decision pending |

The base's `health` keeps two known reds until the lock transition is applied. A dogfood build
from `013b0ab` therefore still links rustls 0.23.45. If PR 4315 reaches `main` before the cut,
README.md's prepared `## Security` entry goes into the `zfb` lane, the candidate SHA moves, and
the CI and size measurements below must be repeated on the new head.

## Remaining qualification

None of these is done at `013b0ab`. Each is an input to the owner's release decision.

| Item | State | Owner / next step |
| --- | --- | --- |
| Darwin evidence and lock apply for the security transition | Mac stage collection approved (after recipe/resource preflight); `--apply` and live admission not approved | L1, owner |
| Linux lock transition | Not applied; the base's two health reds stand until it is | L1, after the Darwin stage |
| Native Mac verification at the candidate | Not run. Last native evidence is per-fix (`92e25ad4`, `a76e4775`, `b605a4de`) and a limited helper receipt at `21ef7c4` (graph lane 43/43); no full native workspace run after `21d6807e` | L3, issue 4294 |
| 4133 (eight `cargo test -p zfb --lib` Mac hangs) | Not reproduced under nextest, unverified | L3 |
| 4305 + 4325 native sharding | On hold until after the next exam (owner, 2026-10-19) | Owner |
| `exam.yml` with the nextest 0.9.140 pin | Not run yet; next scheduled Sat 2026-10-24 | Schedule |
| Security Audit | Last run red at `21d6807e` (issue 4314); green only after rustls reaches `main` | L1 |
| Version-stamped sizes at 4.5.4 | Measured on draft probe [PR 4333](https://github.com/Takazudo/zudo-front-builder/pull/4333) (health run 39321789489, probe head `59310d3`): md-wasm byte-identical to 4.5.3, Linux island raw and `gzipNormalized` totals unchanged, no ceiling moved. Re-pin commit `2e21689` changes two live-lineage assertions in `island-size-budget.test.mjs` and needs a fresh-context review; re-run if the candidate SHA moves | L5; reviewer |
| Release procedure gates | Focused package/Rust checks, docs check, strict docs build, emitted-HTML validation, `release.yml` | Release run, after approval |

## Dogfooding the candidate downstream

Nothing is published, so a downstream project consumes local tarballs. The repository's own
pre-publish smoke, `scripts/smoke-packed-clean-room.sh`, is the reference recipe: it packs the
packages, installs them in a clean room through `pnpm-workspace.yaml` `overrides`, scaffolds and
builds a site, and since PR 4328 (Stage 7) asserts that a store-installed build and a
symlink-linked build of the same packed contents emit byte-identical `dist/`. Follow the same
shape:

1. **Get a binary for `013b0ab`.**
   - Linux x64: the `smoke-binary-amd64` artifact of run
     [39316381226](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381226)
     holds `zfb-bin` and `zfb-css.node`. It is kept for one day only (`retention-days: 1`); after
     that, build it.
   - macOS or any other host: build from source at the SHA (`pnpm install --frozen-lockfile`,
     then `cargo build --release -p zfb` and
     `bash scripts/build-css-node.sh <target>` for the CSS addon). On a low-memory Mac use
     `CARGO_BUILD_JOBS=1` on the command and the heavy guard. This is a Mac build, so it needs a
     free Mac slot; nothing here starts one.
   - Either binary reports a development version, because `ZFB_RELEASE_VERSION` is set only by
     `release.yml`. Do not use `zfb --version` to identify the checkpoint; record the SHA.
2. **Pack at the SHA.** In a checkout of `013b0ab`, run
   `pnpm --filter @takazudo/zfb-slugify --filter @takazudo/zfb --filter @takazudo/zfb-runtime --filter @takazudo/zfb-adapter-cloudflare build`.
   Copy the binary into the platform package (for Linux x64, `packages/zfb-linux-x64-gnu/zfb`
   mode 0755 and `zfb-css.node`), then pack the platform package with `npm pack` (pnpm pack
   strips the exec bit) and the others with `pnpm pack`, exactly as Stages 1–2 of the smoke
   script do. The tarballs carry version 4.5.3, since `main` is not bumped; the bump-probe
   branch of PR 4333 carries 4.5.4 manifests if a downstream needs that version string.
3. **Install downstream.** In the downstream project's `pnpm-workspace.yaml`, add
   `overrides` mapping `@takazudo/zfb`, `@takazudo/zfb-slugify`, `@takazudo/zfb-runtime`,
   `@takazudo/zfb-adapter-cloudflare` (if used) and the platform package to `file:<tarball>`,
   then `pnpm install`. pnpm 11+ ignores `pnpm.overrides` in `package.json`.
4. **Check store-install parity.** Build the downstream site once from the store install and
   once with the same packed contents linked, then compare with
   `scripts/assert-dist-byte-parity.sh --require '<islands glob>' <dist-a> <dist-b>`. That is
   the 4231 property PR 4328 added to CI; a downstream difference is a finding for issue 4231.
5. **Report back** with the downstream project, the SHA, the host, the commands and the first
   result, so the evidence can join the release decision.

## Separate later work: the Deno exit roadmap

These are not 4.5.4 and do not block it. Each phase needs its own owner decision.

| Issue | Phase | State on `main` `013b0ab` |
| --- | --- | --- |
| 4301 | Opt-in direct V8 host in the CLI | Prototype feature only on `main`; the CLI wiring is PR 4307, integrated on `base/4301-direct-v8` `bea94d1`, waiting on lock admission |
| 4302 | Five native targets, packaging, Rolldown output | Linux x64 native row passed at `14d984a`; the other rows, packaging and Rolldown coverage not run |
| 4303 | Default cutover decision and removal of the deno_core host | Not started; owner decision |

When any of these lands on `main` in a later release, that release's notes describe it. The
4.5.4 notes do not mention direct V8 as a feature.
