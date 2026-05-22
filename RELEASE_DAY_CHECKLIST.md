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
