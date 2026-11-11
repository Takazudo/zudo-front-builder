# Release process

## Router regression check (added by issue #1349)

If the release touches `packages/zfb-runtime` router code (client router, view transitions, scroll restoration, bfcache), run `pnpm test:webkit-back` (WebKit back-nav, T4 local-heavy, Mac only, never runs in CI — `pnpm test:router-chromium` already runs in `router-chromium.yml`) before publishing.

## Release channels and GitHub Release assets (added by issue #381, updated by issue #455)

### Release trigger (X9)

The release workflow (`release.yml`) triggers on **`release: published`** — NOT
on a tag push. Choose one complete flow:

1. **Default autonomous:** run `/l-make-release`. It creates a draft with no
   pre-uploaded Mac archive, publishes it, and watches the provenance-first CI
   path. Draft creation itself does NOT trigger the workflow; publishing does.
2. **Autonomous fast-Mac opt-in:** on a Mac, run
   `/l-make-release --fast-mac`. The skill creates the draft, builds + uploads
   the macOS-x64 archive and `.sha256`, publishes, and watches the mixed-provenance
   path. Do not upload or publish a second time afterward.
3. **Manual/confirmed:** run `/l-make-release --confirm`. It stops at the
   unpublished draft. Optionally run `/l-make-mac-release-binary vX.Y.Z` on a
   Mac for the explicit fast path, then publish per
   [stable publish order](#stable-publish-order-withhold-latest-verify-then-promote)
   (a prerelease still uses `gh release edit vX.Y.Z --draft=false`).

### Tagging and channel policy

- Tags matching `*-next.*`, `*-beta.*`, or `*-rc.*` → GitHub Release is created
  with `prerelease: true` and npm dist-tag `next`.
- All other `v*` tags → GitHub Release `prerelease: false` and npm dist-tag `latest`.
- The GitHub "latest release" API automatically skips prereleases, so stable
  installers (install.sh, Homebrew, winget) always get the last non-prerelease
  that was promoted to Latest. A new stable is published without Latest and
  promoted only after delivery verification (below).
- Installer opt-in to a specific prerelease: set `ZFB_VERSION=v0.X.Y-next.N`
  or `ZFB_VERSION=latest-prerelease` before running the installer.

### Stable publish order: withhold Latest, verify, then promote

`install.sh` builds archive URLs from GitHub `/releases/latest` with no
fallback, so a stable Release must not become Latest before its assets, npm
packages and installer path are proven. After the existing preparation and
qualification gates (unchanged):

1. **Publish without Latest in one update.** `make_latest` is a string, and it
   must be set in the same request that clears `draft`. Do not publish as
   Latest and clear it afterward, and do not use a bare
   `gh release edit vX.Y.Z --draft=false` (or the web UI with
   "Set as the latest release" checked) on this path:

   ```sh
   RELEASE_ID=$(gh release view vX.Y.Z --json databaseId --jq .databaseId)
   printf '%s' '{"draft": false, "prerelease": false, "make_latest": "false"}' \
     | gh api -X PATCH "repos/Takazudo/zudo-front-builder/releases/${RELEASE_ID}" --input -
   ```

   This fires the normal `release: published` event while the previous
   completed stable remains Latest.
2. **Observe both runs to completion and record their ids:** the `Release` run
   (event `release`, head branch `vX.Y.Z`), then the
   `Binary smoke, scaffold E2E & showcase` run that it triggers by
   `workflow_run`, whose `Smoke released (install.sh)` job installs the exact
   tag. That job runs only after a successful Release run; after a rerun,
   select the installer run triggered by the final attempt.
3. **Run the read-only verifier** from the release session, after both runs
   complete — never as a `release.yml` step, since it audits that run's
   completion:

   ```sh
   node scripts/verify-release-delivery.mjs --version X.Y.Z \
     --release-run-id <Release run id> \
     --installer-run-id <node-free-smoke workflow_run id> \
     --json <evidence-dir>/release-delivery-X.Y.Z.json
   ```

   Add `--expected-source <40-hex sha>` when the release commit is known. Keep
   the JSON report. The verifier only issues GET requests; it sends
   `GH_TOKEN`/`GITHUB_TOKEN` to `api.github.com` when set. Behind an
   `HTTPS_PROXY`, set `NODE_USE_ENV_PROXY=1`, since Node's `fetch` ignores the
   proxy otherwise. Exit **0** means every required check verified and is the
   only result that permits promotion; **1** is a definite failure, **2** is
   incomplete or unavailable evidence (never a pass) and **3** is a usage or
   internal error. It accepts only the normal provenance-enabled path: fast-Mac
   (mixed provenance), recovery and dry-run runs are classified, never
   accepted, so promoting after them is a separate owner decision.
4. **Promote, then update Homebrew** — only after exit 0 and with the release
   session's authorization (a default `/l-make-release` run carries it):

   ```sh
   printf '%s' '{"make_latest": "true"}' \
     | gh api -X PATCH "repos/Takazudo/zudo-front-builder/releases/${RELEASE_ID}" --input -
   ./scripts/update-homebrew-formula.sh vX.Y.Z --push
   ```

5. **Read back the channels** and report them separately from immutable
   delivery: re-run the verifier with `--expect-github-latest
   --expect-homebrew`, plus `--expect-npm-latest` for a stable that should hold
   npm `latest` (every normal stable).

On exit 1 or a persistent 2, leave the Release published but not Latest, skip
Homebrew, and report the failing checks. Do not unpublish or retag.

`make_latest="false"` controls GitHub Latest only. It does not make the
eleven-package npm publication transactional and does not defer npm's dist-tag
behavior: a stable publish moves `latest` immediately, and `next` is retired
per `release.yml` (see the CLAUDE.md dist-tag rule). Prereleases are unchanged:
they never become GitHub Latest and still publish with
`gh release edit vX.Y.Z --draft=false`.

### Prerelease dual-tag policy (issue #481)

During the prerelease phase, each `*-next.*` publish advances **both** `next`
and `latest` — so `npm install @takazudo/zfb` (no tag) and `npm create zfb`
track the latest prerelease and are never left pointing at a stale release.

**Self-disabling condition**: the `publish` job probes `npm view @takazudo/zfb
dist-tags.latest` before each `*-next.*` publish. If `latest` is empty (first
ever publish) or is itself a prerelease (version string contains `"-"`), the
workflow advances `latest` alongside `next` for all 11 workspace packages. Once
a real stable version holds `latest` the condition is false and prereleases no
longer touch it — a prerelease can never clobber a real stable `latest` after
launch.

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

### What the workflow uploads to the GitHub Release

After all platform builds complete, the `release-assets` job uploads:

| File | Description |
|---|---|
| `zfb-{semver}-aarch64-apple-darwin.tar.gz` | macOS arm64 archive |
| `zfb-{semver}-x86_64-apple-darwin.tar.gz` | macOS x64 archive |
| `zfb-{semver}-x86_64-unknown-linux-gnu.tar.gz` | Linux x64 archive |
| `zfb-{semver}-aarch64-unknown-linux-gnu.tar.gz` | Linux arm64 archive |
| `zfb-{semver}-x86_64-pc-windows-msvc.zip` | Windows x64 archive |
| `*.sha256` (5 files) | One sha256 file per archive |

`{semver}` = value of `packages/zfb/package.json` `.version`, no leading `v`.

Each sha256 file contains exactly one line:

```
<64-hex-lowercase>  <archive-basename>
```

(two spaces between hash and filename — GNU `sha256sum` default).

### Dry-run smoke test

Run `workflow_dispatch` with `dry_run: true` to exercise all build, tarball,
and sha256 steps without uploading to a GH Release or publishing to npm.
The `release-assets` job will list the collected files instead of uploading.

## v5.2.1 incomplete delivery and successor release handoff

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

### Later authorized release operations

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
   [stable publish order](#stable-publish-order-withhold-latest-verify-then-promote).
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

## macOS-x64 local-build escape hatch (added by issue #437, updated by issue #455)

This escape hatch was built because the former Intel macOS runner
(`macos-13`) was chronically queue-starved. That image has since been **retired
outright** (2025-12-04); the x86_64 leg now targets **`macos-15-intel`**, the
migration label. GitHub's current runner table also lists **`macos-26-intel`**;
re-evaluate the selected label when `macos-15-intel` retires in **Fall 2027**.
The cross-compile in `build-macos-x64-local.sh` remains the fallback if no
suitable Intel runner is offered then.

Because the local-build path has been used for every recent release, the dead
`macos-13` label sat unnoticed in the matrix for 8+ releases. It only affects
the no-pre-upload path, where it would have hung the whole release rather than
failing fast — `publish` needs `build`.

The `detect-mac-local` job (A2) auto-detects whether a locally-built macOS-x64
archive has been pre-uploaded to the draft Release before it was published,
so no manual re-dispatch is needed.

For SDK revisions exporting `@takazudo/zfb/css`, the native carrier also needs
`zfb-css.node`. The fast-Mac script emits a separate
`zfb-css-{semver}-x86_64-apple-darwin.node` and `.sha256`; upload both alongside
the unchanged single-binary archive and its checksum. Detection and ingestion
require all four assets. Older recovery tags without the CSS export still use
the original two assets.

The fast-Mac route must load and compile with the x64 addon before upload.
Use x64 Node >=22, setting `ZFB_X64_NODE=/absolute/path/to/x64/node` on a Mac
whose default Node is arm64. Apple Silicon also needs Rosetta for that binary.
A static CLI version scan does not replace this addon execution gate.

### Default path (CI-built, fully attested; no pre-upload)

1. Run `/l-make-release` to create the draft Release.
2. Publish the draft with NO Mac archive attached — a stable one without Latest,
   per [stable publish order](#stable-publish-order-withhold-latest-verify-then-promote).
3. The workflow builds all 5 platforms on CI and publishes all 11 packages with
   full npm `--provenance` (option C). Verify delivery, then promote Latest and
   update Homebrew.

### Fast-Mac path (`--fast-mac` opt-in; mixed provenance)

**Consumer-breaking whenever an earlier version is attested (true since 2.13.0).**
`zfb-darwin-x64` publishes without `--provenance` on this path, which is a
permanent trust downgrade for that package — a consumer on pnpm's
`trust-policy=no-downgrade` cannot install it at all
(`ERR_PNPM_TRUST_DOWNGRADE`). Before any mutation, `/l-make-release --fast-mac`
runs `node scripts/check-provenance-preflight.mjs --version <v> --mode
mac-local`; on a detected downgrade (exit 1) it aborts unless
`--accept-provenance-downgrade` was also passed, and on a registry error
(exit 2) it always aborts. With the flag, the skill writes the acknowledgement
line `<!-- zfb-release: allow-provenance-downgrade -->` into the draft Release
body — `release.yml`'s `mac-local` publish step requires that exact line to be
present and refuses to publish without it.

For an autonomous release on a Mac, run `/l-make-release --fast-mac
--accept-provenance-downgrade` (the flag is required only when the preflight
detects a downgrade; omit it and the skill will tell you if it was needed). It
creates the draft, builds + uploads the macOS-x64 assets, publishes, and
watches the workflow end to end. Do not repeat the upload or publish steps
afterward.

For a manual/confirmed flow where the upload happens separately:

1. Run `/l-make-release --confirm` to create the unpublished draft.
2. On a Mac, choose exactly one way to build + upload the macOS-x64 asset to the
   **draft** Release:

   - Run `/l-make-mac-release-binary vX.Y.Z`, or
   - manage the equivalent upload manually with:

   ```sh
   ./scripts/build-macos-x64-local.sh --upload vX.Y.Z
   ```

   This runs `pnpm install`, builds `x86_64-apple-darwin` in release mode, and
   uploads `zfb-{semver}-x86_64-apple-darwin.tar.gz` + `.sha256` to the draft
   Release for `vX.Y.Z` (`gh release upload --clobber`, so re-runs are safe).
   The archive + checksum are byte-format-identical to what the runner produces
   (same name, same one-file-at-root layout, same GNU two-space sha256 line).

   Before publishing, check whether this upload would downgrade trust:
   `node scripts/check-provenance-preflight.mjs --version X.Y.Z --mode
   mac-local`. On a detected downgrade (exit 1), get an explicit decision, then
   add the acknowledgement line to the draft body before publishing — `gh
   release view vX.Y.Z --json body --jq .body`, append `<!-- zfb-release:
   allow-provenance-downgrade -->` on its own line, and write it back with `gh
   release edit vX.Y.Z --notes-file -`. `/l-make-mac-release-binary` does this
   check for you; a manual upload must do it by hand.
3. **Publish the draft Release** (a stable one without Latest, per
   [stable publish order](#stable-publish-order-withhold-latest-verify-then-promote);
   a prerelease with `gh release edit vX.Y.Z --draft=false`) to trigger
   `release.yml`. The verifier does not accept this mixed-provenance path, so
   promoting the Release to Latest afterward is an explicit owner decision. Without the acknowledgement line present when a
   downgrade applies, the publish job refuses to run the `mac-local` lane. The
   workflow's `detect-mac-local` job will see both
   files on the Release and output `mac_local_present=true`, causing it to:

   - drop the `darwin-x64` build leg from the matrix (no `macos-15-intel` wait),
   - download the pre-uploaded archive, verify its `.sha256`, extract the binary,
   - upload the other 4 platform archives to the Release (the pre-uploaded
     macOS-x64 asset stays in place),
   - publish `@takazudo/zfb-darwin-x64` **without** `--provenance` while every
     other package is published **with** `--provenance` (mixed provenance, option B).

   Half-upload safety: if the `.tar.gz` is present but the `.sha256` is missing,
   `detect-mac-local` emits a warning and falls back to `mac_local_present=false`
   (full CI build). Upload both files to use the fast-Mac path.

### Published-release recovery (`workflow_dispatch`)

If the `release: published` event is lost or the release workflow needs a fix
after the GitHub Release is already public, do **not** move the tag, delete the
Release, or publish npm packages by hand. Commit the workflow fix to `main`, then
dispatch the reviewed workflow from `main` while pointing it at the existing tag:

**Recovery is now always consumer-breaking for `trust-policy=no-downgrade`, for
all 11 packages** — see below for why GitHub OIDC cannot attest the older tag
commit. Unlike fast-Mac (which acknowledges only an actual detected
downgrade), recovery unconditionally omits provenance for every package, so
the `publish` job always requires the acknowledgement in this mode regardless
of what `check-provenance-preflight.mjs --mode recovery-no-provenance` reports.
The `allow_provenance_downgrade` `workflow_dispatch` boolean input (default
`false`) must be set to `true` to acknowledge this; the publish step refuses to
run without it:

```sh
gh workflow run release.yml --ref main \
  -f dry_run=false \
  -f skip_macos_x64=false \
  -f release_tag=vX.Y.Z \
  -f allow_provenance_downgrade=true
```

The `release-context` job fails closed unless `release_tag` is a valid existing
published Release whose tag commit and `packages/zfb` version agree. Every build
job checks out that exact verified commit SHA, while the workflow definition
comes from `main`. This distinction matters: selecting an older tag in the
"Use workflow from" control also selects that tag's old workflow file, so it
cannot recover a bug in that workflow.

Release assets use `gh release upload --clobber` against that verified tag. The
upload is intentionally asset-only: recovery must not PATCH the existing
Release's metadata based on the workflow's `refs/heads/main` trigger context.

`detect-mac-local` queries the specified Release during recovery, so leave
`skip_macos_x64=false` normally. Set it to `true` only as an escape-hatch when
both macOS-x64 assets are already attached; the publish job still downloads and
verifies the checksum before using them.

Recovery packages are published without npm provenance. A workflow dispatched
from `main` has an OIDC identity for the `main` workflow commit, not the older
tag commit that was actually checked out and built; attaching that attestation
would misidentify the source. The publish job loads its recovery helper from a
separate checkout of the exact `main` workflow commit while keeping all package
source and binaries pinned to the verified tag commit. Normal
`release: published` runs retain the full or mixed-provenance behavior below.
Recovery uploads assets only and never changes Latest; because
`verify-release-delivery.mjs` classifies a recovery run rather than accepting
it, promoting a recovered stable that was published without Latest is a
separate owner decision.

#### Recovery creates a trust downgrade — plan the follow-up release (issue #2623)

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

### Provenance trade-off (option B — mixed provenance)

npm `--provenance` (added in #425 / PR #436) requires OIDC attestation from the
GHA job that built the code. A locally-built `darwin-x64` binary did **not** run
inside the GHA job, so it cannot carry provenance. When the fast-Mac path is used
the publish step therefore splits:

- `@takazudo/zfb-darwin-x64` → published **without** `--provenance`
- all other packages → published **with** `--provenance`

This is the "mixed-provenance" option B from issue #437. The default is now option C:
the CI-built, fully attested path. `/l-make-release` takes that path by default;
use `--fast-mac` only as an explicit opt-in when you deliberately accept mixed
provenance.

The local-build path was the norm because `/l-make-release` previously defaulted
to it, not because it was intended as the escape hatch. That default is now
inverted: normal releases use option C, while `--fast-mac` selects option B.
`@takazudo/zfb-darwin-x64` carried no attestation through `2.12.0`, while
`zfb-linux-x64-gnu` carried one on every release through `2.11.0`.

That standing gap is *not* itself a trust downgrade — pnpm's check needs an
earlier attested version to compare against, and this package has none, which is
why only 5 of the 6 `2.12.0` entries were flagged. It was the weaker problem of
macOS-x64 users getting no supply-chain evidence at all. The default `2.13.0`
release restored provenance for this package and expired the temporary legacy
exception; from `2.13.0` onward, any later `--fast-mac` release is correctly a
`regression`, the weekly provenance guard fails, and the pre-publish preflight
(`check-provenance-preflight.mjs --mode mac-local`) refuses to run that release
without an explicit `--accept-provenance-downgrade` — it is **consumer-breaking
for `trust-policy=no-downgrade`**, not a supervised no-op. The publish job warns
on this path too.

## Homebrew tap update (added by issue #383)

**`/l-make-release` now does this for you on its default path** (since v1.1.0,
2026-08-01): after the `release.yml` watch succeeds, Step 11 runs the command
below itself for **stable** releases. The manual flow here still applies when you
publish by hand — a `--confirm` run, a Release published from the web UI, or a
recovery — since nothing is then watching `release.yml` to trigger it.

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
