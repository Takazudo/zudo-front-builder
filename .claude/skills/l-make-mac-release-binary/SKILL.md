---
description: "Explicit --fast-mac escape hatch: build the x86_64-apple-darwin zfb binary locally on a Mac and upload it to the draft GH Release for the tag when the macos-15-intel CI leg must be avoided. Triggers on \"make mac binary\", \"build mac release binary\", \"upload mac binary\". Standalone entry for building on a separate Mac; the normal /l-make-release path delegates to CI for provenance."
user-invocable: true
argument-description: "Required: the tag (e.g. v0.1.0-next.5)"
---

# /l-make-mac-release-binary

Mac-only escape-hatch skill that builds the `x86_64-apple-darwin` zfb binary locally and uploads it to an existing draft GitHub Release. The normal `/l-make-release` path leaves the archive absent so `release.yml` builds it on `macos-15-intel` with provenance. Use this entry point only for the explicit `--fast-mac` choice, which makes `zfb-darwin-x64` publish unattested; because `2.13.0` established its attestation, the weekly drift guard will correctly fail on any later fast-Mac release (that is intended supervision, not a bug).

## When to use this standalone escape hatch

If `/l-make-release` itself runs on a Mac, prefer `/l-make-release --fast-mac`. That autonomous flow creates the draft, builds and uploads the archive, publishes the Release, and watches CI; **do not invoke this standalone skill afterward**.

Use this skill when the local Mac build must happen separately from a manual/confirmed release flow:

1. Run `/l-make-release --confirm` on any host. It creates the draft Release and stops without publishing or pre-uploading the Mac archive.
2. On a Mac, run `/l-make-mac-release-binary v<ver>` — this builds and uploads the archive to that existing draft Release.
3. Publish the draft Release (from any host): `gh release edit v<ver> --draft=false` (or use the web UI). The `release: published` event starts `release.yml`, which detects the pre-uploaded binary and takes the fast publish path.

If the provenance-first default is desired, do not use this skill: publish the draft without a pre-uploaded Mac archive and let the `macos-15-intel` CI leg build it.

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

7. **Rust toolchain present, and `cargo` resolves to rustup (not Homebrew).** Run `rustup --version` (abort if missing). Then run `cargo --version`: if it prints `(Homebrew)` — or `which cargo` points into `/opt/homebrew` — a standalone Homebrew `rust` is shadowing the rustup shims (happens when `~/.cargo/bin` is not ahead of `/opt/homebrew/bin` on PATH). Homebrew's rust is **host-only** and cannot cross-compile `x86_64-apple-darwin`; the build then dies with `error[E0463]: can't find crate for core`. Put the rustup toolchain first for the build:

   ```bash
   export PATH="$HOME/.rustup/toolchains/$(rustup show active-toolchain | awk '{print $1}')/bin:$PATH"
   cargo --version   # must NOT say (Homebrew)
   ```

   The build script runs `rustup target add x86_64-apple-darwin` itself (idempotent). On a **first-ever** install of that target the `rust-std` download can lag behind `cargo build`, producing the same E0463 error on the first run — just confirm `rustup target list --installed` shows `x86_64-apple-darwin` and re-run.

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
skip the macos-15-intel build leg, and publish all 10 packages. Because this is the explicit
`--fast-mac` escape hatch, `zfb-darwin-x64` publishes unattested; because `2.13.0` established its
attestation, the weekly drift guard will correctly fail on any later fast-Mac release. That is
intended supervision, not a bug.

After publishing, WAIT for the Release workflow run to finish (gh run watch) — it uploads the
remaining platform archives (linux + windows) and their .sha256 files.

If v<version> is a STABLE release, update Homebrew ONLY after that run succeeds (the script
404s if any platform's .sha256 is not yet on the Release). SKIP for prereleases — brew tracks
the stable channel; testers use `npm i -g @takazudo/zfb@next` or ZFB_VERSION=latest-prerelease:

  ./scripts/update-homebrew-formula.sh v<version> --push
============================================================
```

## Notes

- This skill does NOT publish the draft Release. The user publishes it afterward via `gh release edit v<ver> --draft=false` or the web UI.
- The archive name is LOCKED — must be exactly `zfb-{semver}-x86_64-apple-darwin.tar.gz` (no leading `v` in semver). `release.yml`'s A2 detection greps for this exact name plus the `.sha256` companion.
- The build script has a fallback that creates a GH Release if none exists. In the explicit `--fast-mac` flow, precondition 5 ensures the intended draft exists before invoking the script, so the fallback does not fire.
- The build script reads the semver from `packages/zfb/package.json` — the tag argument is used only for the `gh release upload` target. The semver in the archive name comes from the package.json, not the tag string.
- **Abandoning the draft.** This skill uploads to an existing draft Release; it never tears one down. If the release is abandoned (problem found, or never published), clean up the orphaned draft via `/l-make-release cancel` (see its "Cancelling a release / cleaning up an orphaned draft" section) — it deletes the draft + the uploaded archive/`.sha256` assets and decides whether to revert the bump.
- **Apple Silicon + no Rosetta 2: the script's `--version` assertion fails.** `build-macos-x64-local.sh` runs `"$built_binary" --version` to verify the `ZFB_RELEASE_VERSION` stamp. On an Apple Silicon host **without Rosetta 2** the x86_64 binary cannot execute and the step dies with `Bad CPU type in executable` (verify Rosetta with `arch -x86_64 /usr/bin/true`). The cross-compile itself has already succeeded by then. Two ways forward:
  - Install Rosetta once (`softwareupdate --install-rosetta --agree-to-license`, needs admin) and re-run — `cargo build` is cached so the re-run is fast.
  - Or, if Rosetta cannot be installed, **verify the stamp statically** and finish the post-build steps by hand. The clap version comes from `option_env!("ZFB_RELEASE_VERSION")` (see `crates/zfb/src/cli.rs`); since the script always injects it, confirm it landed in rodata: `strings <binary> | grep -F "<semver>"` should show the literal next to `zfb`/`zudo-front-builder`. Then replicate the script's tail: `cp` the binary to `packages/zfb-darwin-x64/zfb`, `tar -C packages/zfb-darwin-x64 -czf zfb-<semver>-x86_64-apple-darwin.tar.gz zfb`, write the `.sha256` (GNU two-space format), and `gh release upload <tag> <archive> <archive>.sha256 --clobber`.
