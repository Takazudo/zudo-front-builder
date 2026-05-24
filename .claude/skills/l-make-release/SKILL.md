---
description: "Release @takazudo/zfb — bump the version, write the changelog, push, wait for CI, pre-create the draft GH Release, build the macOS x86_64 binary locally (when on a Mac), and stop before publishing. Triggers on rough requests like \"bump version\", \"cut a release\", \"release zfb\", \"make a release\"."
user-invocable: true
argument-description: "Optional: major, minor, patch, next, stable — controls version bump strategy"
---

# /l-make-release

Orchestrator for releasing `@takazudo/zfb` and its lockstep workspace packages. Bumps the version, writes a changelog doc, commits + pushes, waits for CI, pre-creates a draft GitHub Release, builds + uploads the macOS x86_64 binary locally (when run on a Mac; otherwise notifies and defers that leg to CI), and **stops before publishing**. The user decides when to publish.

## Invocation & confirmation

This skill is **model-invocable**: a rough natural-language request like "bump version", "cut a release", or "release zfb" may trigger it. **It must never mutate anything before the user explicitly confirms.** Steps 1–3 are read-only (preconditions, version computation, change analysis); the first mutation is Step 4. Always present the Step 3 proposal (current → new version + categorized changelog) and **wait for explicit user confirmation** before proceeding to Step 4. If the trigger was a loose phrase, restate the proposed bump plainly so the user can catch a wrong version strategy before anything is written.

The lockstep packages are:

- `@takazudo/zfb` (`packages/zfb/package.json`) — **version source-of-truth**
- `@takazudo/zfb-runtime` (`packages/zfb-runtime/package.json`)
- `@takazudo/zfb-adapter-cloudflare` (`packages/zfb-adapter-cloudflare/package.json`)
- `create-zfb` (unscoped — `packages/create-zfb/package.json`)
- `@takazudo/zfb-darwin-arm64` (`packages/zfb-darwin-arm64/package.json`)
- `@takazudo/zfb-darwin-x64` (`packages/zfb-darwin-x64/package.json`)
- `@takazudo/zfb-linux-arm64-gnu` (`packages/zfb-linux-arm64-gnu/package.json`)
- `@takazudo/zfb-linux-x64-gnu` (`packages/zfb-linux-x64-gnu/package.json`)
- `@takazudo/zfb-win32-x64-msvc` (`packages/zfb-win32-x64-msvc/package.json`)

The workspace root `package.json` is private and stays at `0.0.0` — do NOT bump it or include it in version commits.

The Rust CLI binary is built by `.github/workflows/release.yml`, not by this skill — do not attempt to build it locally.

## Boundaries

- This skill **never** publishes the draft Release. The user runs `gh release edit v<version> --draft=false` themselves.
- This skill **never** pushes a tag separately. The draft Release creation (`gh release create --draft`) creates the tag remotely.
- This skill **never** publishes to npm. `release.yml` does that when the draft is published.

## Step 1: Preconditions

Before doing anything else, verify ALL of the following. If any check fails, stop with a clear message.

1. Current branch is `main` (`git branch --show-current`)
2. Working tree is clean (`git status --porcelain` returns empty)
3. `gh` CLI is authenticated (`gh auth status`)
4. At least one `v*` tag exists (`git tag -l 'v*'`). If no tag exists, tell the user to create the initial tag first (e.g. `git tag v0.1.0 && git push --tags`).

## Step 2: Determine Next Version

Read the current version from `packages/zfb/package.json` (the version source-of-truth — not the workspace root).

Apply the following rules based on the optional argument:

### No argument

- If current is `X.Y.Z-next.N` (prerelease): propose `X.Y.Z-next.{N+1}`
  - Example: `0.1.0-next.3` → `0.1.0-next.4`
- If current is stable `X.Y.Z`: propose `X.{Y+1}.0-next.1`
  - Example: `0.1.0` → `0.2.0-next.1`

### `next` argument (from stable)

- Force-start a new minor prerelease: `X.{Y+1}.0-next.1`
- Example: `0.1.0` → `0.2.0-next.1`

### `major` argument

- Bump major, reset minor+patch, start prerelease: `{X+1}.0.0-next.1`
- Example: `0.1.0-next.5` → `1.0.0-next.1`, `0.1.0` → `1.0.0-next.1`

### `minor` argument

- Bump minor, reset patch, start prerelease: `X.{Y+1}.0-next.1`
- Example: `0.1.0-next.5` → `0.2.0-next.1`, `0.1.0` → `0.2.0-next.1`

### `patch` argument

- Bump patch, start prerelease: `X.Y.{Z+1}-next.1`
- Example: `0.1.0-next.5` → `0.1.1-next.1`, `0.1.0` → `0.1.1-next.1`

### `stable` argument

- Strip the `-next.N` suffix from the current prerelease.
- Requires current version to be a `-next.N` prerelease. If it is stable already, stop with an error.
- Example: `0.1.0-next.5` → `0.1.0`

## Step 3: Analyze Changes and Propose

Find the latest version tag. First fetch remote tags — under the X9 flow, prior releases created their `v*` tag only on GitHub (via the draft `gh release create`), so the most recent tag may be absent from this local checkout. Without the fetch, `git tag -l` picks a stale older tag and the changelog base re-includes already-released commits:

```bash
git fetch --tags origin
git tag -l 'v*' --sort=-v:refname | head -1
```

Analyze commits since that tag:

```bash
git log <last-tag>..HEAD --oneline
```

Categorize each commit by its conventional-commit prefix:

- **Breaking Changes**: commits with `!` suffix (e.g. `feat!:`) or `BREAKING CHANGE` in body
- **Features**: `feat:` prefix
- **Bug Fixes**: `fix:` prefix
- **Other Changes**: everything else (`docs:`, `chore:`, `refactor:`, `ci:`, `test:`, `style:`, `perf:`, etc.)

Present the proposal to the user:

```
Proposed bump: {current} → {new} ({type})

Breaking Changes:
- description (hash)

Features:
- description (hash)

Bug Fixes:
- description (hash)

Other Changes:
- description (hash)
```

Only show sections that have entries. **Wait for user confirmation before proceeding.**

## Step 4: Bump + Sync + Changelog mdx

### 4a. Update packages/zfb/package.json

Update the `version` field in `packages/zfb/package.json` to the confirmed new version (without the `v` prefix). Do NOT touch the workspace root `package.json`.

### 4b. Propagate to lockstep packages

```bash
node scripts/sync-platform-versions.mjs
```

This propagates the new version to all lockstep packages and updates `optionalDependencies` in `packages/zfb/package.json`.

### 4c. Regenerate lockfile

```bash
pnpm install
```

This regenerates `pnpm-lock.yaml` so CI's `pnpm install --frozen-lockfile` succeeds.

**Lockfile drift heuristic** — before staging, run (the goal is to surface added/removed lines that are NOT simple two-space-indented entries; `grep -E` cannot do a negative lookahead, so use `grep -P` where available, else the awk fallback):

```bash
# PCRE (GNU grep -P / ripgrep): show +/- lines that are NOT two-space-indented
git diff pnpm-lock.yaml | grep -P '^[+-](?!  )' | head -20
# Portable fallback (BSD/macOS grep lacks -P):
# git diff pnpm-lock.yaml | awk '/^[+-]/ && !/^[+-]  / { print } ' | head -20
```

If you see non-version-related changes (structural changes, unexpected lines), stop and surface the diff to the user before proceeding.

### 4d. Write changelog mdx

Create `docs/src/content/docs/changelog/v<version>.mdx`:

```mdx
---
title: 'v<version>'
sidebar_position: <computed>
---

# v<version>

Released: <YYYY-MM-DD>

## Breaking Changes

- description (hash)

## Features

- description (hash)

## Bug Fixes

- description (hash)

## Other Changes

- description (hash)
```

Rules:

- Only include sections that have entries.
- Use today's date for `Released`.
- Each entry: commit subject followed by the short hash in parentheses.
- `sidebar_position` formula: `MAJOR*10000 + MINOR*1000 + PATCH*10 + (prereleaseN || 10)`
  - Stable versions use `10` in the prerelease slot so they sort **above** their own prereleases when the category uses `sortOrder: "desc"`.
  - Examples: `0.1.0` (stable) = `0*10000 + 1*1000 + 0*10 + 10` = **1010**; `0.1.0-next.5` = `0 + 1000 + 0 + 5` = **1005**; `0.1.0-next.1` = **1001**.
  - Desc-sorted: 1010 > 1005 > 1001 → stable appears above its prereleases, newer prereleases above older ones.

## Step 5: Build + Test (focused)

```bash
pnpm --filter @takazudo/zfb test && cargo test --package zfb
```

If anything fails, stop and tell the user. Do not proceed.

Note: the Rust CLI binary is built by `.github/workflows/release.yml` — do not attempt to build it here.

## Step 6: Atomic Commit + Push

Stage and commit all bumped files atomically in a **single commit**:

```bash
git add packages/*/package.json pnpm-lock.yaml crates/zfb/src/commands/new.rs docs/src/content/docs/changelog/v<version>.mdx
git commit -m "chore(release): bump to v<version>"
git push origin main
```

Note: `git add` of an unchanged file (e.g. `crates/zfb/src/commands/new.rs` when the Rust source doesn't embed the version) is a no-op — include it in the add command regardless.

Record the resulting commit SHA:

```bash
BUMP_SHA=$(git rev-parse HEAD)
```

## Step 7: Wait for CI on Bump Commit

Delegate CI polling to the `/watch-ci` skill — do NOT reimplement polling:

```
Skill(skill="watch-ci", args="--branch main --commit <bump-sha>")
```

If CI fails, fix the issue, re-push, then re-invoke `/watch-ci` before proceeding.

## Step 8: Detect Partial State (previous run)

Before creating the draft Release, check whether a release for this version already exists:

```bash
gh release view v<version> 2>/dev/null
```

If it exists, present the user with three options and wait for their choice:

- **Reuse**: skip the `gh release create` step and proceed to the notify message.
- **Delete and recreate**: `gh release delete v<version> --yes --cleanup-tag` then re-create.
- **Abort**: stop.

Also verify that the most-recent commit on `main` matches the version in the mdx (the `Released:` date and the filename `v<version>.mdx` should align with the current HEAD). If there is a mismatch, surface it and recommend rollback before proceeding.

## Step 9: Pre-create Draft GH Release

```bash
NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/v<version>.mdx)
PRERELEASE_FLAG=$([[ "<version>" =~ -next\.|-beta\.|-rc\. ]] && echo "--prerelease" || echo "")
gh release create v<version> --target <bump-sha> --title "v<version>" --notes "$NOTES" --draft $PRERELEASE_FLAG
```

The tag is created remotely as a draft. The `release: published` webhook event does NOT fire on draft creation (by design).

## Step 10: Build the macOS x86_64 Binary Locally (default)

GitHub's `macos-13` runners are slow and frequently queue-starved, so this skill builds the Mac binary **locally by default** and pre-uploads it to the draft Release. `release.yml`'s detect-mac-local job then skips the `macos-13` leg at publish time.

Detect the host OS:

```bash
uname -s
```

### If `Darwin` (macOS)

Build and upload directly via the locked-contract script. The orchestrator is already on `main` at the bump commit with a clean tree, so re-running the `/l-make-mac-release-binary` preconditions would be redundant — **call the script directly**:

```bash
./scripts/build-macos-x64-local.sh --upload v<version>
```

Then verify BOTH assets are attached and read the checksum for the report:

```bash
gh release view v<version> --json assets --jq '.assets[].name'
awk '{print $1}' "zfb-<version>-x86_64-apple-darwin.tar.gz.sha256"
```

Both `zfb-<version>-x86_64-apple-darwin.tar.gz` and its `.sha256` companion must appear. If either is missing, stop and surface what was found vs. expected.

### If NOT `Darwin`

Do not attempt the build (the cross-target needs an Apple host). **Notify the user** with this message, then continue to Step 11:

```
⚠️  Not a macOS host (uname = <result>). Skipping the local Mac build.
    To build the Mac binary locally (recommended — saves the slow macos-13 CI leg),
    run on a Mac before publishing:

      /l-make-mac-release-binary v<version>

    Otherwise, publishing the draft will let CI build the macos-13 leg (slower).
```

## Step 11: Notify + STOP

Print the message below **verbatim** (substitute the actual version string for `<version>`), picking the block that matches whether the Mac archive was uploaded in Step 10. Do not paraphrase command strings or URLs.

The Homebrew step is gated to **stable** releases (it tracks the stable channel, like npm `latest`). If `<version>` is a prerelease (`-next.` / `-beta.` / `-rc.`), do NOT run `update-homebrew-formula.sh` — direct prerelease testers to `npm i -g zfb@next` or the curl installer's `ZFB_VERSION=latest-prerelease`.

### If the Mac binary was built + uploaded (Step 10 on macOS)

````
============================================================
Release bump committed and pushed.
CI on the bump commit: PASSED.
Draft GH Release created: v<version> (tag exists remotely as a draft).
macOS x86_64 binary: built locally and uploaded to the draft Release.

NEXT STEP — publish the draft to trigger release.yml (from any host):

  gh release edit v<version> --draft=false
  # or via the web UI: https://github.com/Takazudo/zudo-front-builder/releases

release.yml's detect-mac-local job will see the pre-uploaded archive,
skip the slow macos-13 leg, and publish all 9 packages.

After publishing, WAIT for the Release workflow run to finish — it builds and uploads the
remaining platform archives (linux + windows) and their .sha256 files, then publishes the
npm packages:

  gh run watch

If this is a STABLE release, update Homebrew once the Release run above succeeds (the script
fetches every platform's .sha256 from the Release and 404s if it has not finished). SKIP this
for prereleases — brew tracks the stable channel; testers use `npm i -g zfb@next` or the curl
installer with ZFB_VERSION=latest-prerelease:

  ./scripts/update-homebrew-formula.sh v<version> --push

(See RELEASE_DAY_CHECKLIST.md for the Homebrew flow — not handled by this skill.)
============================================================
````

### If the Mac build was skipped (Step 10 not on macOS)

````
============================================================
Release bump committed and pushed.
CI on the bump commit: PASSED.
Draft GH Release created: v<version> (tag exists remotely as a draft).
macOS x86_64 binary: NOT built (host is not macOS).

NEXT STEP — pick one:

Option A (recommended): build the Mac binary on a Mac first, then publish

  On a Mac (zfb checkout, on main at the bump commit):

    /l-make-mac-release-binary v<version>

  Then publish (from any host):

    gh release edit v<version> --draft=false

Option B: publish now and let CI build the macos-13 leg (slower)

    gh release edit v<version> --draft=false
    # or via the web UI

Either way, release.yml auto-detects whether the Mac archive is on the Release at
publish time. If present → skip macos-13 (fast). If absent → build on CI.

After publishing, WAIT for the Release workflow run to finish — it builds and uploads the
remaining platform archives (linux + windows, and the macos-13 leg under Option B) and their
.sha256 files, then publishes the npm packages:

  gh run watch

If this is a STABLE release, update Homebrew once the Release run above succeeds (the script
fetches every platform's .sha256 from the Release and 404s if it has not finished). SKIP this
for prereleases — brew tracks the stable channel; testers use `npm i -g zfb@next` or the curl
installer with ZFB_VERSION=latest-prerelease:

  ./scripts/update-homebrew-formula.sh v<version> --push
============================================================
````

Then **STOP**. Do NOT publish the draft from this skill.

## Failure Recovery

### pnpm-lock.yaml drift

Run `git diff pnpm-lock.yaml | grep -E '^[+-](?!  )' | head -20` before staging. If non-version-line structural changes appear, stop and surface the diff. Fix the lockfile manually before re-running.

### Build or test failure (Step 5)

Stop and report the failure. Do not proceed with the commit. Fix the issue and re-run the skill.

### CI fails on bump commit (Step 7)

Fix the issue, commit the fix, push, then re-invoke `/watch-ci`. Do not proceed to the draft Release until CI is green.

### Existing draft Release for the version (Step 8)

Prompt: reuse / delete-and-recreate / abort. Wait for user choice before acting.

### Mismatched mdx + commit (Step 8)

Surface the mismatch clearly. Recommend: roll back with `git revert <bump-sha> && git push origin main`, delete the orphan changelog mdx, then re-run `/l-make-release`. Wait for user decision.

### Rolling back the bump

Instruct the user:

```bash
git revert <bump-sha>
git push origin main
rm docs/src/content/docs/changelog/v<version>.mdx
```

Then re-run `/l-make-release` from the start.
