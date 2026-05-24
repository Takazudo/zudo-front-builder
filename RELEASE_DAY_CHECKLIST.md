## Release-day checklist (out of scope for this PR — flip manually)

Each package below has `private: true` and (for two of them) a
`*-migration.0` version string. Before publishing each one to npm:

- `packages/zfb` — bump `version` (currently `0.0.0`), remove `"private": true`, then `pnpm publish --access public`.
- `packages/zfb-runtime` — bump `version` (currently `0.2.0-migration.0`), remove `"private": true`, then `pnpm publish --access public`.
- `packages/zfb-adapter-cloudflare` — bump `version` (currently `0.1.0-migration.0`), remove `"private": true`, then `pnpm publish --access public`.

Sub 7b (this PR) prepared the surrounding metadata — descriptions,
keywords, repository / homepage / bugs / author / license / files
allowlists, `publishConfig.access: "public"`, READMEs as finished
npmjs.com landing pages, and CHANGELOGs — but did **not** touch
`version` or `private` per the issue body's explicit non-scope.

## Release channels and GitHub Release assets (added by issue #381)

### Tagging and channel policy

- Push a tag matching `v*` to trigger a release build.
- Tag matching `*-next.*`, `*-beta.*`, or `*-rc.*` → GitHub Release is created
  with `prerelease: true` and npm dist-tag `next`.
- All other `v*` tags → GitHub Release `prerelease: false` and npm dist-tag `latest`.
- The GitHub "latest release" API automatically skips prereleases, so stable
  installers (install.sh, Homebrew, winget) always get the last non-prerelease.
- Installer opt-in to a specific prerelease: set `ZFB_VERSION=v0.X.Y-next.N`
  or `ZFB_VERSION=latest-prerelease` before running the installer.

### What the workflow uploads to the GitHub Release

After all 5 platform builds complete, the `release-assets` job uploads:

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

## macOS-x64 local-build escape hatch (added by issue #437)

The GHA `macos-13` (legacy Intel) runner is chronically queue-starved — GitHub
is throttling its capacity to push users toward Apple Silicon images. It is the
single point of failure for releases: the other 4 platforms build in 5–15 min,
while `darwin-x64` has repeatedly stalled for hours. This escape hatch lets a
Mac dev host build that one artifact locally and skip the runner entirely.

### Default path (no escape hatch)

Push a `v*` tag and wait for the full 5-platform GHA matrix. Use this whenever
`macos-13` is behaving — it keeps full npm `--provenance` on every package.

### Escape-hatch path (when `macos-13` is stuck)

1. **Push the `vX.Y.Z` tag as usual.** The Release workflow starts.
2. **If the `darwin-x64` (macos-13) leg stalls**, cancel that workflow run. The
   GH Release for the tag has already been created (or will be — if not, the
   other platforms' run created it).
3. **On a Mac, build + upload the macOS-x64 asset:**

   ```sh
   ./scripts/build-macos-x64-local.sh --upload vX.Y.Z
   ```

   This runs `pnpm install`, builds `x86_64-apple-darwin` in release mode, and
   uploads `zfb-{semver}-x86_64-apple-darwin.tar.gz` + `.sha256` to the GH
   Release for `vX.Y.Z` (`gh release upload --clobber`, so re-runs are safe).
   The archive + checksum are byte-format-identical to what the runner produces
   (same name, same one-file-at-root layout, same GNU two-space sha256 line).
4. **Re-dispatch `release.yml` against the tag with the escape hatch on:**
   - Actions → Release → "Run workflow"
   - **Use workflow from:** select the **tag** `vX.Y.Z` (not a branch — the
     publish/upload guards require a `refs/tags/v*` ref)
   - `dry_run`: `false`
   - `skip_macos_x64`: `true`

   With `skip_macos_x64=true` the workflow:

   - drops the `darwin-x64` build leg from the matrix (no `macos-13` wait),
   - verifies the macOS-x64 archive + `.sha256` are already on the GH Release
     before the publish job proceeds (fails loud if missing),
   - uploads the other 4 archives to the Release (the pre-uploaded macOS-x64
     asset stays in place),
   - publishes all npm packages, splitting the publish so
     `@takazudo/zfb-darwin-x64` is published **without** `--provenance` while
     every other package keeps `--provenance` (see "Provenance trade-off"
     below).

### Provenance trade-off (option B — mixed provenance)

npm `--provenance` (added in #425 / PR #436) requires OIDC attestation from the
GHA job that built the code. A locally-built `darwin-x64` binary did **not** run
inside the GHA job, so it cannot carry provenance. When `skip_macos_x64=true`
the publish step therefore splits:

- `@takazudo/zfb-darwin-x64` → published **without** `--provenance`
- all other packages → published **with** `--provenance`

This is the "mixed-provenance" option B from issue #437. Prefer the default path
(option C: only use the escape hatch when `macos-13` is actually stuck) so most
releases keep full provenance across every package.

## Homebrew tap update (added by issue #383)

After the GitHub Release assets are published, regenerate and push the Homebrew
tap formula. Requires a local checkout of `Takazudo/homebrew-tap` at the default
path (`~/repos/myoss/homebrew-tap`):

```sh
./scripts/update-homebrew-formula.sh vX.Y.Z --push
```

The script fetches the sha256 checksums from the GH Release assets (the `.sha256`
files produced by the release workflow), writes `Formula/zfb.rb` in the tap, and
commits + pushes `"zfb X.Y.Z"`.

**Note:** No seed commit is needed for the first release — running the command
above on the first real `vX.Y.Z` tag will both create `Formula/zfb.rb` and push
it to the tap for the first time.
