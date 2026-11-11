# Release day checklist

**The canonical release procedure is [RELEASING.md](RELEASING.md).** This file keeps
release-day policy notes, the Release asset layout and preserved incident history; it
does not duplicate the procedure. When the two disagree, `RELEASING.md` wins.

## Router regression check (added by issue #1349)

If the release touches `packages/zfb-runtime` router code (client router, view transitions, scroll restoration, bfcache), run `pnpm test:webkit-back` (WebKit back-nav, T4 local-heavy, Mac only, never runs in CI — `pnpm test:router-chromium` already runs in `router-chromium.yml`) before publishing.

## Release channels and GitHub Release assets

### Release trigger

`release.yml` triggers on **`release: published`** — never on a tag push. The
procedure creates one **draft** Release (which fires nothing and has no tag ref
yet), the owner's Mac uploads the Darwin inputs to it, and publishing the draft
starts `release.yml` from the tag. There is never a second Release for a version.
Recovery and finalize runs are explicit `workflow_dispatch` runs from `main`
([RELEASING.md](RELEASING.md#workflow-dispatches-recovery-finalize-dry-run)).

npm publication is split: the unprivileged `pack` job (`Pack npm tarballs`)
installs, builds, runs the CSS/binary carrier preflight (moved there from the
publish step) and packs every tarball; the `publish` job holds the OIDC
`id-token`, installs and builds nothing, and runs `npm publish <tgz>` per
package with explicit provenance. pnpm never publishes
([RELEASING.md](RELEASING.md#provenance-contract-and-the-downgrade-blocker)).

### Tagging and channel policy

- Tags matching `*-next.*`, `*-beta.*`, or `*-rc.*` → GitHub Release is created
  with `prerelease: true` and npm dist-tag `next`.
- All other `v*` tags → GitHub Release `prerelease: false` and npm dist-tag `latest`.
- The GitHub "latest release" API automatically skips prereleases, so stable
  installers (install.sh, Homebrew, winget) always get the last non-prerelease
  that was promoted to Latest.
- **A stable is published without Latest and promoted only after delivery
  verification.** `install.sh` builds archive URLs from `/releases/latest` with
  no fallback. Publish with `draft=false`, `prerelease=false` and
  `make_latest="false"` in one REST update, run `scripts/verify-release-delivery.mjs`,
  and promote only on its exit 0 ([RELEASING.md](RELEASING.md) A12–A15).
  `make_latest` controls GitHub Latest only: a stable npm publish still moves
  `latest` immediately, and `next` is retired per the CLAUDE.md dist-tag rule.
- Installer opt-in to a specific prerelease: set `ZFB_VERSION=v0.X.Y-next.N`
  or `ZFB_VERSION=latest-prerelease` before running the installer.

### Prerelease dual-tag policy (issue #481)

> The forward half of this policy is self-disabled since v1.0.0 took `latest`
> (a prerelease no longer moves `latest`); it is kept for the remediation commands.

During the prerelease phase, each `*-next.*` publish advances **both** `next`
and `latest` — so `npm install @takazudo/zfb` (no tag) and `npm create zfb`
track the latest prerelease and are never left pointing at a stale release.

**Self-disabling condition**: `scripts/release/resolve-also-latest.mjs`
decides, in the `publish` and `finalize` jobs, before each `*-next.*` dist-tag
step. It advances `latest` alongside `next` for all 11 packages only when
`@takazudo/zfb` has no `latest` — proven by a definitive 404 (first publish) —
or its `latest` is itself a prerelease. Any other registry failure (5xx, auth,
network, malformed body) fails closed (exit 2) and is never read as "first
publish". `scripts/advance-latest-dist-tag.sh` additionally refuses to move a
stable `latest` to a prerelease. Once a real stable holds `latest`,
prereleases no longer touch it.

`release.yml` authenticates every npm write (publish, `dist-tag add`,
`dist-tag rm`) through GitHub OIDC trusted publishing only; there is no npm
token secret to create or rotate. Each of the 11 packages needs a trusted
publisher on npmjs.com for `Takazudo/zudo-front-builder`, workflow
`release.yml`, no environment, with **Allow npm dist-tag** enabled (it is off
by default, and without it the dist-tag writes fail with an auth error).

The retry logic in `release.yml` already retries each `npm dist-tag add` up to
5 times with exponential backoff, and never moves `latest` to a lower version.
If that exhausts, the workflow prints a manual remediation command per package,
to run from a maintainer's own interactive npm session. For the two
user-facing install surfaces:

```sh
npm dist-tag add @takazudo/zfb@0.1.0-next.6 latest
npm dist-tag add create-zfb@0.1.0-next.6 latest
```

Run the equivalent commands for the remaining 8 packages if the workflow log
shows failures across all of them (substitute the actual version for
`0.1.0-next.6`).

### What the GitHub Release holds

The owner's Mac uploads the Darwin inputs to the draft before publishing
([RELEASING.md](RELEASING.md#phase-m-darwin-inputs-on-the-owners-mac));
`release.yml` admits them and uploads the rest:

| File | Producer |
|---|---|
| `zfb-{semver}-aarch64-apple-darwin.tar.gz` (+ `.sha256`) | owner's Mac, native arm64 |
| `zfb-{semver}-x86_64-apple-darwin.tar.gz` (+ `.sha256`) | owner's Mac, qualified Rosetta-translated |
| `zfb-css-{semver}-{darwin triple}.node` (+ `.sha256`) | owner's Mac, one per Darwin target |
| `zfb-{semver}-{darwin triple}.qualification.json` | owner's Mac, one receipt per Darwin target |
| `zfb-{semver}-x86_64-unknown-linux-gnu.tar.gz` (+ `.sha256`) | `release.yml` |
| `zfb-{semver}-aarch64-unknown-linux-gnu.tar.gz` (+ `.sha256`) | `release.yml` |
| `zfb-{semver}-x86_64-pc-windows-msvc.zip` (+ `.sha256`) | `release.yml` |
| `zfb-md-wasm-{semver}-shipped-artifacts.json` | `release.yml`, after matching the published npm tarball |

`{semver}` = value of `packages/zfb/package.json` `.version`, no leading `v`.
`scripts/release/release-status.mjs --tag v{semver}` prints the exact expected set
and which assets are present, missing or conflicting. Assets are never clobbered.
The Darwin contract is [scripts/darwin-artifact-contract.md](scripts/darwin-artifact-contract.md).

Each sha256 file contains exactly one line:

```
<64-hex-lowercase>  <archive-basename>
```

(two spaces between hash and filename — GNU `sha256sum` default).

### Dry-run smoke test

Dispatch from a `base/` or `feature/` branch with `dry_run=true` passed
explicitly (its default is false) to exercise the build, tarball and sha256
steps without uploading Release assets or publishing to npm. It still uploads
Actions artifacts and may send the CI failure alert, validates only that source,
and never proves OIDC publication or dist-tag writes.

## Provenance on release day

npm publishes through GitHub OIDC trusted publishing only; there is no npm token
secret. Both Darwin carrier packages are built on the owner's Mac, so they publish
with `--provenance=false`; every other package publishes with `--provenance`.
Earlier Darwin versions stay attested (arm64 since 1.0.0, x64 since 2.13.0), so
**every** such release is a trust downgrade that only the owner can acknowledge —
per run (Release-body marker or dispatch input) or by an owner-committed
`release/provenance-policy.json` — see
[RELEASING.md](RELEASING.md#provenance-contract-and-the-downgrade-blocker) and
[research/npm-oidc-cutover.md](research/npm-oidc-cutover.md).

### Recovery creates a trust downgrade — plan the follow-up release (issue #2623)

> Since #4690 this applies to a `recovery-no-provenance` run (control commit ≠
> source commit). A recovery dispatched while `main`'s head is still the release
> commit uses the normal `mac-local` mode with honest provenance.

**A recovery run is not consequence-free for consumers.** npm records provenance
per published version and versions are immutable, so publishing a version without
an attestation *after* earlier versions had one is a permanent **trust downgrade**
for every affected package. A consumer who has enabled pnpm's `trustPolicy:
no-downgrade` cannot install such a version at all:

```text
ERR_PNPM_TRUST_DOWNGRADE High-risk trust downgrade for "@takazudo/zfb@X.Y.Z" (possible package takeover)
```

This is easy to miss on release day, and v2.12.0 shipped it unnoticed. Measured
against the live registry on 2026-08-27, the blast radius depends on the
consumer's pnpm major:

| Install | pnpm 10.21 – 11.1 | pnpm 11.2+ |
| --- | --- | --- |
| fresh / forced resolve | fails | fails |
| `pnpm install --frozen-lockfile` | **succeeds** (resolution skipped) | **fails** |

pnpm 11.2 added a verification pass that re-applies the active supply-chain
policies to every entry of an already-resolved lockfile, so a committed lockfile
pinning the bad version breaks there while the same lockfile keeps an older CI
job green. That split is why nothing went red on release day.

The publish job now emits a `::warning` and a job-summary block on every
provenance-omitting run so the downgrade cannot ship silently again — do not
dismiss it.

The setting is opt-in (default `off`) and available since pnpm 10.21. It is
`trustPolicy: no-downgrade` in `pnpm-workspace.yaml` (honored by both majors);
pnpm 10 also accepts the older `trust-policy=no-downgrade` in `.npmrc`, which
**pnpm 11 ignores** — pnpm 11 reads only auth/registry settings from `.npmrc`.

**Required follow-up after any recovery run:** ship the next patch through the
normal tag/release path so provenance is restored. That is the only remedy — the
recovered version itself can never be repaired. Until that patch is out, affected
consumers need explicit guidance; the user-facing workaround is documented in
[the docs troubleshooting page](docs/src/content/docs/guides/troubleshooting.mdx)
under `ERR_PNPM_TRUST_DOWNGRADE`.

## Historical: macOS-x64 escape hatch and hosted Intel leg (#437, #455)

> **Historical record.** Retired by #4687. Do not follow these steps.

Until #4687, `release.yml` built `x86_64-apple-darwin` on a hosted Intel runner
(`macos-13`, then `macos-15-intel`) by default, with a `--fast-mac` escape hatch
(`scripts/build-macos-x64-local.sh`, `/l-make-release --fast-mac`, the
`detect-mac-local` job and the `skip_macos_x64` dispatch input) that pre-uploaded a
locally built x64 archive and published `zfb-darwin-x64` unattested ("option B,
mixed provenance"). `zfb-darwin-x64` carried no attestation through 2.12.0;
2.13.0 restored it, and v2.20.3's fast-Mac publish was the downgrade that
`check-provenance-preflight.mjs` (#3134) was written to block. That script's
`--clobber` upload, single-target pair and lack of qualification receipts do not
satisfy the current Darwin contract; it now refuses `--upload` (exit 2) and remains only
for `css-carrier-verification.yml`. Both Darwin targets now come only from
`scripts/build-darwin-local.sh` on the owner's Mac, and no hosted macOS runner is
part of a release.

## Historical: v5.2.1 incomplete delivery and successor release handoff

> **Historical record (2026-11).** Preserved evidence only. The Intel job, the
> `skip_macos_x64` input and the hosted-macOS release legs described here were
> removed by #4687; current steps are in [RELEASING.md](RELEASING.md).

The [v5.2.1 release run](https://github.com/Takazudo/zudo-front-builder/actions/runs/42117295889)
left a public Release with no assets. Its native Intel job built the CLI in 37m26s;
the separate CSS addon build was cancelled after 5m14s when the whole job reached
its 45-minute deadline. Compilation was still progressing, and no compiler or
network failure was established. The comparable v5.2.0 Intel job completed in
38m07s (25m43s CLI, 8m52s addon). Workflow, scripts, runner image and Rust
inputs were unchanged in that comparison. The new 90-minute Intel job allowance
is proposed cold-build margin, not a measured guarantee; other targets retain
45 minutes. The reason for the throughput difference is unknown.

A successful Intel verification must include the CLI and separate CSS addon
compilations, addon load/compile smoke, CLI version assertion, archive and
checksum generation, and both Actions artifact uploads. A CLI build alone does
not establish delivery. The [workflow-repair rehearsal](https://github.com/Takazudo/zudo-front-builder/actions/runs/42150967296)
passed on attempt 1 at source `dcd2e077106b6ee43b4775e93ead134032945064`.
The [Intel job](https://github.com/Takazudo/zudo-front-builder/actions/runs/42150967296/job/127673979858)
used `macos-15-intel`, image `macos-15` version `20260824.0482.1` (macOS
15.7.9), and rustc 1.99.1. CLI compilation took 13m43s, addon compilation
5m36s, and the full job 21m18s. Addon smoke, version assertion, archive,
checksum and both uploads passed. The binary artifact is `13473242761`
(SHA-256 `29cdd2ee776593be468b9c007d7fb6cb5a21da2918c9f10acb9f60ce7e5e123f`);
the archive artifact is `13472833661` (SHA-256
`899ca159a878427d82edb5d7604a9eb48ace46e79399dff40d5ddc4ec23d5250`).
All platform and WASM jobs passed. The dry-run asset and publish jobs passed
without uploading or publishing; clean-room smoke was correctly skipped. This
one successful run validates the workflow on its named source, not a guarantee
of future cold-build duration. Subsequent changes touched only the offline test
and this evidence, not the rehearsed workflow or build inputs.

A branch dispatch with `dry_run=true`, `skip_macos_x64=false`, an empty
`release_tag`, and `allow_provenance_downgrade=false` exercises the workflow
without publishing packages or Release assets. It uploads Actions artifacts and
may send the existing CI failure alert. It validates the workflow at its named
source, not the version-stamped measurements or final release gates for a later
version. Never omit `dry_run`: its default is false. Use a `base/` or `feature/`
ref, not a ref beginning with `v`.

### Later authorized release operations (historical)

These are a handoff, not authorization to mutate production during development.
Refresh main, releases, tag, assets, registry versions and concurrent release
state before acting. Preserve the public v5.2.1 Release and tag, failed run,
and [measurement probe #4601](https://github.com/Takazudo/zudo-front-builder/pull/4601)
unmerged until its own accepted delivery closeout. Keep unrelated trackers open.

1. **Restore the standalone default only after authorization.** If GitHub
   `/releases/latest` still selects asset-free v5.2.1, first verify v5.2.0 is
   the last completed stable release. The proposed metadata-only operation is
   `PATCH /repos/Takazudo/zudo-front-builder/releases/432899648` with
   `{"make_latest":"true"}`. Re-read Latest after the change. The installers
   construct archive URLs from Latest without fallback. Refresh npm state rather
   than inventing a registry rollback; the observed npm latest was 5.2.0.
2. **Land the repair after separate production authorization.** Refresh main,
   reconcile intervening changes, and use the normal review and checks. A main
   push deploys the showcase; do not use an administrator bypass as routine
   repair. The corrected workflow must be in the successor's release commit.
3. **Prepare the proposed stable v5.2.2 from a newly frozen base.** Recheck
   version availability and use a new disposable exact-version probe, not #4601.
   Obtain fresh Linux measurements for all four version-stamped MD/WASM variants
   (`finalWasm`, `gzip9`, `glue`, `glueGzip9`), exports, packing and existing
   budgets; fresh Linux island-size evidence with actual run/job/SHA/lock
   identities and the `--reason release-bump` helper; and the updated Wind
   compiler-source digest, Wind assets and Docs gate. Preserve all raw and
   normalized gzip ceilings. Carry the complete measured bump atomically and
   satisfy the release procedure's final-commit gates. Reuse prior product
   validation only where its source, platform and fingerprints justify it; use
   the owner's established Mac for required native owner verification and
   release orchestration. CI Intel packaging is not native Intel acceptance.
   Successor release notes must describe the intended v5.2.1 fixes: npm users
   are effectively upgrading from 5.2.0. Never relabel 5.2.1 measurements as
   5.2.2 evidence.
4. **Publish through the normal provenance-enabled release event.** A retry of
   the old v5.2.1 tag selects its old 45-minute workflow; a main-based recovery
   for that tag omits provenance. Do not retag or delete v5.2.1, substitute
   unattested local Intel assets, override GitHub identity, publish npm manually,
   or invent an attestation. For the authorized successor, publish the draft
   with `draft=false`, `prerelease=false`, and `make_latest="false"` in the
   **same update**, firing the normal release event while the last completed
   release remains the default. After delivery verification, promote the new
   Release to Latest. Do not publish it as Latest and clear it afterward. This
   is now the normal
   [stable publish order](RELEASING.md#a12-publish-the-draft).
5. **Verify delivery before closeout.** Require all five CLI archives and five
   checksum companions with correct bytes and layout, native CSS addons in
   relevant platform npm packages, all eleven exact npm versions with integrity,
   truthful provenance identity and correct dist-tags, all four published WASM
   digests matching both the package manifest and verified Release asset, and
   exact-version clean-room scaffold/build smoke. Verify both npm publication and
   Release assets: they are sibling jobs. An existing-version skip in the
   publish helper proves existence, not complete integrity or provenance. Check
   Latest and installer URLs, then perform and verify one stable Homebrew update
   with formula and tap commit evidence. `scripts/verify-release-delivery.mjs`
   checks these items for the exact version, Release run and installer run;
   only its exit 0 permits the Latest promotion, and its channel read-back is
   reported separately. Close disposable probes unmerged only
   after their own accepted delivery closeout and preserve reachable receipts.

## Homebrew tap update (added by issue #383)

[RELEASING.md](RELEASING.md) A15 runs this for **stable** releases after the
delivery verifier's exit 0 and the Latest promotion. Run it by hand when nothing
is watching `release.yml` — a `--confirm` run, a Release published from the web
UI, or a recovery — or when the release session lacks tap push access.

After the GitHub Release assets are published (for the normal stable path:
after the verifier's exit 0 and the Latest promotion), regenerate and push the Homebrew
tap formula. Requires a local checkout of `Takazudo/homebrew-tap` at the default
path (`~/repos/zp/homebrew-tap`, overridable with `ZFB_TAP_PATH`):

```sh
./scripts/update-homebrew-formula.sh vX.Y.Z --push
```

The script fetches the sha256 checksums from the GH Release assets (the `.sha256`
files produced by the release workflow), writes `Formula/zfb.rb` in the tap, and
commits + pushes `"zfb X.Y.Z"`.

**Note:** No seed commit is needed for the first release — running the command
above on the first real `vX.Y.Z` tag will both create `Formula/zfb.rb` and push
it to the tap for the first time.
