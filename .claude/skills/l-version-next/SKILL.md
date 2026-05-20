---
description: "Publish a prerelease version under the npm \"next\" dist-tag"
user-invocable: true
disable-model-invocation: true
argument-description: "Optional: major, minor, or patch to set the base version (default: minor)"
---

# /l-version-next

Publish a prerelease version of `@takazudo/zfb` under the npm `next` dist-tag.

Users install with `npm install @takazudo/zfb@next`.

The lockstep packages are:

- `@takazudo/zfb` (`packages/zfb/package.json`) — **version source-of-truth**
- `@takazudo/zfb-runtime` (`packages/zfb-runtime/package.json`)
- `@takazudo/zfb-adapter-cloudflare` (`packages/zfb-adapter-cloudflare/package.json`)
- `create-zfb` (unscoped — `packages/create-zfb/package.json`)
- `@takazudo/zfb-darwin-arm64` (`packages/zfb-darwin-arm64/package.json`)
- `@takazudo/zfb-darwin-x64` (`packages/zfb-darwin-x64/package.json`)
- `@takazudo/zfb-linux-x64-gnu` (`packages/zfb-linux-x64-gnu/package.json`)
- `@takazudo/zfb-win32-x64-msvc` (`packages/zfb-win32-x64-msvc/package.json`)

The workspace root `package.json` is private and stays at `0.0.0` — do NOT bump it or include it in version commits.

The Rust CLI binary is built by `.github/workflows/release.yml`, not by these skills — do not attempt to build it locally.

## Version Format

`X.Y.Z-next.N` where:

- `X.Y.Z` is the target stable version (e.g., `0.5.0`)
- `N` is an incrementing prerelease number (1, 2, 3, ...)

## Preconditions

Before doing anything else, verify ALL of the following. If any check fails, stop and tell the user.

1. Working tree is clean (`git status --porcelain` returns empty)
2. `gh` CLI is authenticated

## Determine Version

Read the current version from `packages/zfb/package.json` (the version source-of-truth — not the workspace root).

### If current version is already a `-next.N` prerelease:

Increment the prerelease number:

- `0.5.0-next.1` → `0.5.0-next.2`
- `0.5.0-next.2` → `0.5.0-next.3`

### If current version is a stable release (e.g., `0.4.3`):

Determine the base version for the next release:

- If the user passed `major`: bump major (e.g., `0.4.3` → `1.0.0-next.1`)
- If the user passed `minor` or no argument: bump minor (e.g., `0.4.3` → `0.5.0-next.1`)
- If the user passed `patch`: bump patch (e.g., `0.4.3` → `0.4.4-next.1`)

Present the version to the user and **wait for confirmation**.

## Build and Test

```bash
pnpm -r build && pnpm -r test
```

If anything fails, stop and tell the user. Do not proceed.

Note: the Rust CLI binary is built by `.github/workflows/release.yml` — do not attempt to build it here.

## Update Version

1. Update the `version` field in `packages/zfb/package.json` to the new prerelease version. This is the version source-of-truth — do NOT update the workspace root `package.json`.

2. Run the sync helper to propagate the new version to all lockstep packages:

   ```bash
   node scripts/sync-platform-versions.mjs
   ```

3. Regenerate `pnpm-lock.yaml` so the bumped `optionalDependencies` specifiers are recorded:

   ```bash
   pnpm install
   ```

4. Stage and commit all lockstep package.json files plus the regenerated lockfile atomically. The workspace root `package.json` must NOT be included:

   ```bash
   git add packages/*/package.json pnpm-lock.yaml
   git commit -m "chore: Bump version to v{VERSION}"
   ```

## Push

```bash
git push
```

## Publish with `next` Tag

**Ask the user for confirmation before publishing.**

The user will run `npm publish --tag next` manually (it requires browser-based 2FA). Tell the user to run:

```bash
npm publish --tag next
```

After publishing, verify:

```bash
npm view @takazudo/zfb dist-tags
```

This should show both `latest` (stable) and `next` (prerelease) tags.

## Notes

- No changelog doc is created for prerelease versions
- No git tag is created for prerelease versions
- No GitHub release is created for prerelease versions
- To promote a next version to stable, use `/l-version-promote`
- To install the next version: `npm install @takazudo/zfb@next`
