---
description: "Build the x86_64-apple-darwin zfb binary locally on a Mac and upload it to the draft GH Release for the tag, saving the slow macos-13 CI leg. Triggers on \"make mac binary\", \"build mac release binary\", \"upload mac binary\". Standalone entry for building on a separate Mac — /l-make-release builds the Mac binary inline when it already runs on macOS."
user-invocable: true
argument-description: "Required: the tag (e.g. v0.1.0-next.5)"
---

# /l-make-mac-release-binary

Mac-only skill that builds the `x86_64-apple-darwin` zfb binary locally and uploads it to an existing draft GitHub Release. Works with the `/l-make-release` (X9) flow to pre-upload the Mac archive so the `release.yml` publish step can skip the slow `macos-13` CI leg.

## Pair with /l-make-release (X9 workflow)

1. Run `/l-make-release` on any host — bumps version, commits, pre-creates draft Release.
2. Run `/l-make-mac-release-binary v<ver>` on your Mac — builds + uploads the archive to the existing draft Release.
3. Publish the draft Release (from any host): `gh release edit v<ver> --draft=false` (or via web UI). Fires `release: published` → `release.yml` runs → detects the pre-uploaded binary → fast publish.

## Preconditions

Before doing anything else, verify ALL of the following. Abort with a clear message if any check fails.

1. **macOS only** — run `uname -s` and verify it returns `Darwin`. If not, abort:

   ```
   ERROR: /l-make-mac-release-binary must run on macOS (Darwin).
   Current platform: <result of uname -s>
   ```

2. **Tag argument required** — the skill must be invoked as `/l-make-mac-release-binary v<ver>`. If no argument is provided, abort:

   ```
   ERROR: tag argument is required.
   Usage: /l-make-mac-release-binary v<ver>  (e.g. v0.1.0-next.5)
   ```

3. **Working tree clean, on `main` at the bump commit** — verify:

   ```bash
   git status --porcelain   # must return empty
   git branch --show-current  # must return "main"
   ```

   Abort if either check fails.

4. **`gh` CLI authenticated** — run `gh auth status`. Abort if not authenticated.

5. **Draft GH Release exists for the tag** — run:

   ```bash
   gh release view <tag> --json isDraft --jq '.isDraft'
   ```

   The result must be `true`. If the release does not exist or is not a draft, abort:

   ```
   ERROR: No draft GH Release found for <tag>.
   Run /l-make-release first to create the bump commit and the draft Release,
   then re-run this skill on your Mac.
   ```

6. **Build script exists** — verify `scripts/build-macos-x64-local.sh` is present. Abort if missing.

7. **Rust toolchain present** — run `rustup --version`. Abort if rustup is not installed. (The build script runs `rustup target add x86_64-apple-darwin` itself — that step is idempotent.)

## Run the build and upload

Invoke the existing locked-contract script. The script handles `pnpm install --frozen-lockfile`, the Rust build, packaging as `zfb-<semver>-x86_64-apple-darwin.tar.gz` (locked contract — archive name must match `release.yml` exactly), SHA-256 in GNU coreutils format, and `gh release upload --clobber`.

```bash
./scripts/build-macos-x64-local.sh --upload <tag>
```

Where `<tag>` is the argument passed to this skill (e.g. `v0.1.0-next.5`).

## Verify upload

After the script returns successfully, confirm BOTH the archive and its checksum are attached to the Release:

```bash
gh release view <tag> --json assets --jq '.assets[].name'
```

Both of the following must appear in the output:

- `zfb-<semver>-x86_64-apple-darwin.tar.gz`
- `zfb-<semver>-x86_64-apple-darwin.tar.gz.sha256`

Where `<semver>` is the version without the leading `v` (e.g. `0.1.0-next.5`).

If either file is missing, abort with an error listing what was found vs. what was expected.

## Read the SHA-256 hash

After verifying the upload, read the hash from the `.sha256` file to include in the report. The hash is the first field (64 hex characters) from the checksum file. You can retrieve it from the local file written by the script (at the repo root by default):

```bash
awk '{print $1}' "zfb-<semver>-x86_64-apple-darwin.tar.gz.sha256"
```

## Report

Print this exact message (substituting `<version>` with the semver string without the leading `v`, and `<hash>` with the 64-character hex SHA-256 hash):

```
============================================================
Mac binary uploaded to draft Release v<version>.

Archive: zfb-<version>-x86_64-apple-darwin.tar.gz
SHA-256: <hash>

Next: publish the draft Release (from any host) to trigger CI publish:

  gh release edit v<version> --draft=false
  # or via web UI: https://github.com/Takazudo/zudo-front-builder/releases

release.yml will auto-detect the pre-uploaded archive,
skip the macos-13 build leg, and publish all 9 packages.

After publishing, WAIT for the Release workflow run to finish (gh run watch) — it uploads the
remaining platform archives (linux + windows) and their .sha256 files. ONLY after it succeeds,
update Homebrew (the script 404s if any platform's .sha256 is not yet on the Release):

  ./scripts/update-homebrew-formula.sh v<version> --push
============================================================
```

## Notes

- This skill does NOT publish the draft Release. The user publishes it afterward via `gh release edit v<ver> --draft=false` or the web UI.
- The archive name is LOCKED — must be exactly `zfb-{semver}-x86_64-apple-darwin.tar.gz` (no leading `v` in semver). `release.yml`'s A2 detection greps for this exact name plus the `.sha256` companion.
- The build script has a fallback that creates a GH Release if none exists. With the X9 flow, `/l-make-release` creates the draft first, so the fallback does not fire — precondition 5 above ensures the draft exists before invoking the script.
- The build script reads the semver from `packages/zfb/package.json` — the tag argument is used only for the `gh release upload` target. The semver in the archive name comes from the package.json, not the tag string.
