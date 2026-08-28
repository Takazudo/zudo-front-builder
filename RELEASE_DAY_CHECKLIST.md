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
   Mac for the explicit fast path, then publish with
   `gh release edit vX.Y.Z --draft=false` (or the web UI).

### Tagging and channel policy

- Tags matching `*-next.*`, `*-beta.*`, or `*-rc.*` → GitHub Release is created
  with `prerelease: true` and npm dist-tag `next`.
- All other `v*` tags → GitHub Release `prerelease: false` and npm dist-tag `latest`.
- The GitHub "latest release" API automatically skips prereleases, so stable
  installers (install.sh, Homebrew, winget) always get the last non-prerelease.
- Installer opt-in to a specific prerelease: set `ZFB_VERSION=v0.X.Y-next.N`
  or `ZFB_VERSION=latest-prerelease` before running the installer.

### Prerelease dual-tag policy (issue #481)

During the prerelease phase, each `*-next.*` publish advances **both** `next`
and `latest` — so `npm install @takazudo/zfb` (no tag) and `npm create zfb`
track the latest prerelease and are never left pointing at a stale release.

**Self-disabling condition**: the `publish` job probes `npm view @takazudo/zfb
dist-tags.latest` before each `*-next.*` publish. If `latest` is empty (first
ever publish) or is itself a prerelease (version string contains `"-"`), the
workflow advances `latest` alongside `next` for all 10 workspace packages. Once
a real stable version holds `latest` the condition is false and prereleases no
longer touch it — a prerelease can never clobber a real stable `latest` after
launch.

The retry logic in `release.yml` already retries each `npm dist-tag add` up to
5 times with exponential backoff. If that exhausts, the workflow prints a manual
remediation command per package. For the two user-facing install surfaces:

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

### Default path (CI-built, fully attested; no pre-upload)

1. Run `/l-make-release` to create the draft Release.
2. Publish the draft (`gh release edit vX.Y.Z --draft=false` or web UI) with
   NO Mac archive attached.
3. The workflow builds all 5 platforms on CI and publishes all 10 packages with
   full npm `--provenance` (option C).

### Fast-Mac path (`--fast-mac` opt-in; mixed provenance)

For an autonomous release on a Mac, run `/l-make-release --fast-mac`. It creates
the draft, builds + uploads the macOS-x64 assets, publishes, and watches the
workflow end to end. Do not repeat the upload or publish steps afterward.

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
3. **Publish the draft Release** (`gh release edit vX.Y.Z --draft=false` or web UI)
   to trigger `release.yml`. The workflow's `detect-mac-local` job will see both
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

```sh
gh workflow run release.yml --ref main \
  -f dry_run=false \
  -f skip_macos_x64=false \
  -f release_tag=vX.Y.Z
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
`regression` and the weekly provenance guard fails. That is intended supervision
of a deliberate opt-in, not a hazard. The publish job warns on this path too.

## Homebrew tap update (added by issue #383)

**`/l-make-release` now does this for you on its default path** (since v1.1.0,
2026-08-01): after the `release.yml` watch succeeds, Step 11 runs the command
below itself for **stable** releases. The manual flow here still applies when you
publish by hand — a `--confirm` run, a Release published from the web UI, or a
recovery — since nothing is then watching `release.yml` to trigger it.

After the GitHub Release assets are published, regenerate and push the Homebrew
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
