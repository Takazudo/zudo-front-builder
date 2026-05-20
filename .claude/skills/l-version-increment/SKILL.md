---
description: Bump package version, generate changelog doc, tag, and publish to npm
user-invocable: true
disable-model-invocation: true
argument-description: "Optional: major, minor, or patch to skip the proposal step"
---

# /l-version-increment

Bump the version of `@takazudo/zfb`, generate a changelog doc page, commit, tag, and publish to npm.

The version bump must update all eight lockstep packages and the `optionalDependencies` in `packages/zfb/package.json` in lockstep, so that consumers of the published package always resolve a prebuilt binary whose version matches the root package. The `scripts/sync-platform-versions.mjs` script does this mechanically — always run it as part of the bump and commit all package.json files atomically.

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

Before doing anything else, verify ALL of the following. If any check fails, stop and tell the user.

1. Current branch is `main`
2. Working tree is clean (`git status --porcelain` returns empty)
3. At least one `v*` tag exists (`git tag -l 'v*'`). If no tag exists, tell the user to create the initial tag first (e.g. `git tag v0.1.0 && git push --tags`).

Find the latest version tag:

```bash
git tag -l 'v*' --sort=-v:refname | head -1
```

## Analyze changes since last tag

Run:

```bash
git log <last-tag>..HEAD --oneline
```

and

```bash
git diff <last-tag>..HEAD --stat
```

Categorize each commit by its conventional-commit prefix:

- **Breaking Changes**: commits with an exclamation mark suffix (e.g. `feat!:`) or BREAKING CHANGE in body
- **Features**: `feat:` prefix
- **Bug Fixes**: `fix:` prefix
- **Other Changes**: everything else (`docs:`, `chore:`, `refactor:`, `ci:`, `test:`, `style:`, `perf:`, etc.)

## Propose version bump

Based on the changes:

- If there are breaking changes → propose **major** bump
- If there are features (no breaking) → propose **minor** bump
- Otherwise → propose **patch** bump

If the user passed an argument (`major`, `minor`, or `patch`), use that directly instead of proposing.

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

## Create changelog doc

Create `docs/src/content/docs/changelog/v{VERSION}.mdx` with this format:

```mdx
---
title: 'v{VERSION}'
sidebar_position: { computed }
---

# v{VERSION}

Released: {YYYY-MM-DD}

## Breaking Changes

- Description (commit-hash)

## Features

- Description (commit-hash)

## Bug Fixes

- Description (commit-hash)

## Other Changes

- Description (commit-hash)
```

Rules:

- Only include sections that have entries
- `sidebar_position` = `MAJOR * 1000 + MINOR * 100 + PATCH` — the changelog category uses `sortOrder: "desc"`, so higher values appear first (newer versions on top)
- Use today's date for the release date
- Each entry should be the commit subject with the short hash in parentheses

## Commit changelog

```bash
git add docs/src/content/docs/changelog/v{VERSION}.mdx
git commit -m "docs: Add changelog for v{VERSION}"
```

## Bump version in package.json

1. Update the `version` field in `packages/zfb/package.json` to the new version (without the `v` prefix). This is the version source-of-truth — do NOT update the workspace root `package.json`.

2. Run the sync helper to propagate the new version to all lockstep packages and `optionalDependencies` in `packages/zfb/package.json`:

   ```bash
   node scripts/sync-platform-versions.mjs
   ```

3. Regenerate `pnpm-lock.yaml` so the bumped `optionalDependencies` specifiers are recorded. This MUST be done before the commit — CI runs `pnpm install --frozen-lockfile` and will fail if the lockfile lags behind `package.json`.

   ```bash
   pnpm install
   ```

4. Stage and commit all lockstep package.json files plus the regenerated lockfile atomically. The workspace root `package.json` stays at `0.0.0` and must NOT be included:

   ```bash
   git add packages/*/package.json pnpm-lock.yaml
   git commit -m "chore: Bump version to v{VERSION}"
   ```

## Build and test

Run the full build and test suite to make sure everything is good:

```bash
pnpm -r build && pnpm -r test
```

If anything fails, stop and tell the user. Do not proceed with tagging or publishing.

Note: the Rust CLI binary is built by `.github/workflows/release.yml` — do not attempt to build it here.

## Push and wait for CI

Push the commits first (without the tag) and wait for CI to pass:

```bash
git push
```

Then check CI status with `gh run list --branch main --limit 2`. Poll every 30 seconds until both CI and Production Deploy show `completed success`. If CI fails, fix the issue, commit, and push again before proceeding.

**Do not tag or publish until CI is green.**

## Tag, push tag, and create GitHub release

**Ask the user for confirmation before tagging.**

```bash
git tag v{VERSION}
git push --tags
```

After pushing the tag, create a GitHub release using the changelog content (with YAML frontmatter and `# v{VERSION}` heading stripped, since the release title already shows the version):

```bash
NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/v{VERSION}.mdx)
gh release create v{VERSION} --title "v{VERSION}" --notes "$NOTES"
```

## Publish to npm

**Ask the user for confirmation before publishing.**

The user will run `npm publish` manually (it requires browser-based 2FA). Tell the user to run:

```bash
npm publish
```

After publishing, verify the package page: `https://www.npmjs.com/package/@takazudo/zfb`
