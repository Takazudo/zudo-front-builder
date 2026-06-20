# Release process

## Release channels and GitHub Release assets (added by issue #381, updated by issue #455)

### Release trigger (X9)

The release workflow (`release.yml`) triggers on **`release: published`** — NOT
on a tag push. The flow is:

1. Run `/l-make-release` (creates a **draft** GitHub Release for the current
   version tag). Draft Releases do NOT trigger the workflow.
2. Optionally: on a Mac, run `./scripts/build-macos-x64-local.sh --upload vX.Y.Z`
   to pre-upload the macOS-x64 archive + `.sha256` to the draft Release (fast-Mac
   path — skips the chronically slow `macos-13` runner).
3. **Publish the draft Release** (`gh release edit vX.Y.Z --draft=false` or the
   web UI) to trigger `release.yml`. Publishing fires the `release: published`
   event; the draft state does NOT.

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
workflow advances `latest` alongside `next` for all 9 workspace packages. Once
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

Run the equivalent commands for the remaining 7 packages if the workflow log
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

The GHA `macos-13` (legacy Intel) runner is chronically queue-starved — GitHub
is throttling its capacity to push users toward Apple Silicon images. The
`detect-mac-local` job (A2) auto-detects whether a locally-built macOS-x64
archive has been pre-uploaded to the draft Release before it was published,
so no manual re-dispatch is needed.

### Default path (no pre-upload)

1. Run `/l-make-release` to create the draft Release.
2. Publish the draft (`gh release edit vX.Y.Z --draft=false` or web UI) with
   NO Mac archive attached.
3. The workflow builds all 5 platforms on CI and publishes all packages with
   full npm `--provenance`. Use this path whenever `macos-13` is behaving.

### Fast-Mac path (when `macos-13` is slow or you want to skip it)

1. Run `/l-make-release` to create the draft Release.
2. On a Mac, build + upload the macOS-x64 asset to the **draft** Release:

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

   - drop the `darwin-x64` build leg from the matrix (no `macos-13` wait),
   - download the pre-uploaded archive, verify its `.sha256`, extract the binary,
   - upload the other 4 platform archives to the Release (the pre-uploaded
     macOS-x64 asset stays in place),
   - publish `@takazudo/zfb-darwin-x64` **without** `--provenance` while every
     other package is published **with** `--provenance` (mixed provenance, option B).

   Half-upload safety: if the `.tar.gz` is present but the `.sha256` is missing,
   `detect-mac-local` emits a warning and falls back to `mac_local_present=false`
   (full CI build). Upload both files to use the fast-Mac path.

### Manual escape-hatch override (`workflow_dispatch`)

If you need to invoke the workflow manually (e.g. for debugging) and have already
uploaded the macOS-x64 files:

- Actions → Release → "Run workflow"
- **Use workflow from:** select the **tag** `vX.Y.Z`
- `dry_run`: `false`
- `skip_macos_x64`: `true` (forces `mac_local_present=true` without querying the Release)

### Provenance trade-off (option B — mixed provenance)

npm `--provenance` (added in #425 / PR #436) requires OIDC attestation from the
GHA job that built the code. A locally-built `darwin-x64` binary did **not** run
inside the GHA job, so it cannot carry provenance. When the fast-Mac path is used
the publish step therefore splits:

- `@takazudo/zfb-darwin-x64` → published **without** `--provenance`
- all other packages → published **with** `--provenance`

This is the "mixed-provenance" option B from issue #437. Prefer the default path
(option C: only use the escape hatch when `macos-13` is actually stuck) so most
releases keep full provenance across every package.

## Homebrew tap update (added by issue #383)

After the GitHub Release assets are published, regenerate and push the Homebrew
tap formula. Requires a local checkout of `Takazudo/homebrew-tap` at the default
path (`~/repos/Takazudo/homebrew-tap`):

```sh
./scripts/update-homebrew-formula.sh vX.Y.Z --push
```

The script fetches the sha256 checksums from the GH Release assets (the `.sha256`
files produced by the release workflow), writes `Formula/zfb.rb` in the tap, and
commits + pushes `"zfb X.Y.Z"`.

**Note:** No seed commit is needed for the first release — running the command
above on the first real `vX.Y.Z` tag will both create `Formula/zfb.rb` and push
it to the tap for the first time.
