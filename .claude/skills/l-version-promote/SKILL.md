---
description: Promote a next prerelease to a stable release
user-invocable: true
disable-model-invocation: true
---

# /l-version-promote

Promote a `@takazudo/zfb@next` prerelease to a stable release.

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

## Preconditions

1. Current branch is `main`
2. Working tree is clean
3. Current version in `packages/zfb/package.json` (the version source-of-truth — not the workspace root) is a `-next.N` prerelease (e.g., `0.5.0-next.3`)

If the current version is NOT a prerelease, stop and tell the user.

## Determine Stable Version

Strip the `-next.N` suffix:

- `0.5.0-next.3` → `0.5.0`
- `1.0.0-next.1` → `1.0.0`

Present to the user and **wait for confirmation**.

## Create Changelog Doc

Create `docs/src/content/docs/changelog/v{VERSION}.mdx` using the same format as `/l-version-increment`.

Analyze all commits since the last stable tag (`git tag -l 'v*' --sort=-v:refname` excluding prerelease tags) and categorize them.

Rules:

- `sidebar_position` = `MAJOR * 1000 + MINOR * 100 + PATCH`
- Include `title` in frontmatter
- Only include sections that have entries

```bash
git add docs/src/content/docs/changelog/v{VERSION}.mdx
git commit -m "docs: Add changelog for v{VERSION}"
```

## Update Version

1. Update `packages/zfb/package.json` version to the stable version (without `-next.N`). This is the version source-of-truth — do NOT update the workspace root `package.json`.

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

## Build and Test

```bash
pnpm -r build && pnpm -r test
```

If anything fails, stop.

Note: the Rust CLI binary is built by `.github/workflows/release.yml` — do not attempt to build it here.

## Push and Wait for CI

```bash
git push
```

Wait for CI to pass.

## Tag and Release

**Ask for confirmation.**

```bash
git tag v{VERSION}
git push --tags
```

Create GitHub release:

```bash
NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/v{VERSION}.mdx)
gh release create v{VERSION} --title "v{VERSION}" --notes "$NOTES"
```

## Publish to npm (stable)

**Ask for confirmation.**

The user runs manually:

```bash
npm publish
```

This publishes under the `latest` tag (default). The `next` tag automatically becomes stale — users who had installed `@next` will stay on the prerelease version until they explicitly update.

After publishing, verify:

```bash
npm view @takazudo/zfb dist-tags
```

Both `latest` and `next` should show the correct versions.
