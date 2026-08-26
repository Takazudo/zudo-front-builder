---
description: "Release @takazudo/zfb — bump the version, write the changelog, push, wait for CI, pre-create the draft GH Release, build the macOS x86_64 binary locally (when on a Mac), publish the Release, watch release.yml to completion, and push the updated Homebrew formula to the tap on a stable release. STABLE-BY-DEFAULT: with no argument it judges the level from the commits and lands a patch or minor straight on the npm `latest` tag, fully autonomously in one cycle; it stops to ask only when the commits contain a breaking change (major), offering stable-now vs a `next` soak. Pass --confirm to vet the proposal interactively and stop at the unpublished draft. Triggers on rough requests like \"bump version\", \"cut a release\", \"release zfb\", \"make a release\"."
user-invocable: true
argument-description: "Optional — with NO argument the level is judged from the commits and a patch/minor lands stable on `latest` automatically (a major stops and asks). next — force a prerelease instead (publishes to the npm `next` tag): from a stable it starts a -next.1 at the judged level, from a prerelease it continues the line (-next.N+1). major|minor|patch — force a prerelease on that specific component. stable — promote the current prerelease by stripping its suffix. stable major|minor|patch — force a specific stable bump. --confirm — interactive mode: present the bump proposal and wait, and stop at the unpublished draft instead of publishing. Or: cancel — abort/teardown, deletes an orphaned draft GH Release instead of bumping."
---

# /l-make-release

Orchestrator for releasing `@takazudo/zfb` and its lockstep workspace packages. Bumps the version, writes five package-specific changelog docs, commits + pushes, waits for CI, pre-creates a draft GitHub Release, builds + uploads the macOS x86_64 binary locally (when run on a Mac; otherwise the macos-15-intel CI leg builds it at publish time), then **publishes the Release** — triggering `release.yml` (remaining platform binaries + npm publish) — watches that run to completion, and on a **stable** release pushes the updated Homebrew formula to the tap. With `--confirm`, it instead stops at the unpublished draft and the user decides when to publish (and runs Homebrew by hand).

## Invocation & autonomy

This skill is **model-invocable**: a rough natural-language request like "bump version", "cut a release", or "release zfb" may trigger it.

**End-to-end means end-to-end.** On the default path a stable release needs no manual follow-up: the skill publishes, watches `release.yml`, and pushes the Homebrew formula itself (Step 11). Do not end a stable run by telling the user to go run the tap script — run it.

**Default: fully autonomous end-to-end — NEVER ask for confirmation, NEVER stop and wait.** Steps 1–3 are read-only (preconditions, version computation, change analysis); print the Step 3 proposal (current → new version + categorized changelog) **for visibility only** and proceed straight into Step 4 without waiting. The edge cases that used to prompt have autonomous defaults defined inline (Step 1 orphaned drafts, Step 8 partial state). There is no stopping point: Step 11 **publishes the Release itself** and watches the triggered `release.yml` run to completion. Do not pause to ask "publish?", "go?", or any equivalent — the invocation itself is the authorization.

**The single exception: a no-argument MAJOR.** When Step 2's no-argument judgment lands on **major** (the commit range contains a breaking change), stop and ask whether to land it stable or soak it on `next` first — see [Step 2's no-argument rule](#no-argument--judge-the-level-land-stable-unless-it-is-a-major). This is a *version-strategy* question, not a "go?" confirmation, and it is the only one. Once the user answers, the rest of the run is autonomous again as normal. A no-argument **patch** or **minor** never pauses: it lands stable on `latest` without asking.

**`--confirm` option (opt-in interactive mode).** When the invocation includes `--confirm` (e.g. `/l-make-release --confirm`, `/l-make-release minor --confirm`), restore the interactive behavior: present the Step 3 proposal and **wait for explicit user confirmation** before the first mutation (Step 4), ask before acting on the Step 1 / Step 8 edge cases, and **stop at Step 11 with the draft unpublished** — the user publishes manually. Use this when the version strategy or release notes need vetting. Without this flag, do NOT pause anywhere.

**Cancel mode.** Invoking `/l-make-release cancel` — or a request like "cancel the release", "abort the release", "remove the draft" — does NOT bump anything. It jumps straight to ["Cancelling a release / cleaning up an orphaned draft"](#cancelling-a-release--cleaning-up-an-orphaned-draft) below to tear down a leftover draft GH Release. This is the documented escape hatch for the failure mode where a prior run created a draft (Step 9) and stopped before publishing (Step 11), but the release was then abandoned — leaving the draft orphaned on GitHub. Orphaned drafts never fire the `release: published` webhook so they are harmless to CI, but they accumulate and skew the partial-state detection of the next run.

The lockstep packages are:

- `@takazudo/zfb` (`packages/zfb/package.json`) — **version source-of-truth**
- `@takazudo/zfb-runtime` (`packages/zfb-runtime/package.json`)
- `@takazudo/zfb-md-wasm` (`crates/zfb-md-wasm/npm/package.json`)
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

- **Default**: this skill publishes the Release itself at Step 11 (`gh release edit v<version> --draft=false`) once the Mac asset situation is settled, then watches the triggered `release.yml` run to completion. With `--confirm`, it stops at the unpublished draft and the user publishes manually.
- This skill **never** pushes a tag separately. The draft Release creation (`gh release create --draft`) creates the tag remotely.
- This skill **never** publishes to npm directly. `release.yml` does that when the Release is published.
- **Homebrew is automatic on the default path, for stable releases only.** Once `release.yml`
  succeeds, Step 11 runs `./scripts/update-homebrew-formula.sh v<version> --push` itself. Prereleases
  skip it (brew tracks the stable channel). On the `--confirm` path it stays manual — that path stops
  before publishing, so the skill never observes `release.yml` finishing and cannot know the assets
  are up.

## Step 1: Preconditions

Before doing anything else, verify ALL of the following. If any check fails, stop with a clear message.

1. Current branch is `main` (`git branch --show-current`)
2. Working tree is clean (`git status --porcelain` returns empty)
3. `gh` CLI is authenticated (`gh auth status`)
4. At least one `v*` tag exists (`git tag -l 'v*'`). If no tag exists, tell the user to create the initial tag first (e.g. `git tag v0.1.0 && git push --tags`).
5. **No orphaned draft GH Release is silently lingering.** A draft from an abandoned prior run never publishes, but it accumulates and skews Step 8's partial-state detection. List drafts:

   ```bash
   gh release list --json name,isDraft,tagName --jq '.[] | select(.isDraft) | .tagName'
   ```

   If this prints any tag, a draft for a version **other than** the one you are about to release is an orphan from an abandoned run. (A draft for the version you are about to release is handled later in Step 8.)

   - **Default (autonomous)**: delete the orphan(s) per ["Cancelling a release / cleaning up an orphaned draft"](#cancelling-a-release--cleaning-up-an-orphaned-draft) — never-published drafts have no tag ref and no consumers — then report what was deleted and continue.
   - **With `--confirm`**: surface the orphan(s) and offer to delete; wait for the user before continuing.

## Step 2: Determine Next Version

Read the current version from `packages/zfb/package.json` (the version source-of-truth — not the workspace root).

Every rule below sets two independent things: **which component bumps** (major / minor / patch) and
**which channel the result lands on**. The `-next.N` forms publish to the npm `next` tag and leave
`latest` untouched; the `stable` forms publish to `latest` — the version a bare `npm i @takazudo/zfb`,
`brew install`, or `curl | sh` resolves to.

**Stable-by-default.** The no-argument path judges the level from the commits and, for **patch** and
**minor**, lands it **stable** — straight to `latest`, in one release cycle. Prerelease-first is NOT
the default: burning two full cycles (each republishing all 10 lockstep packages, each waiting on a
~40-minute `release.yml`) to ship a routine fix batch is exactly the cost this avoids.

**A major is the one exception** — see the no-argument rule below. That is where a soak on `next`
earns its keep, because a major asserts "this breaks existing projects" and `latest` is immutable
once published.

Explicit arguments override the judgment in both directions: `next` / `major` / `minor` / `patch`
force a prerelease when you *do* want dogfooding; `stable <level>` forces a specific stable bump.
Prefix-derived levels measure the **shape** of a change, not its **risk** — when a batch is shaped
like a minor but smells like a soak (a subsystem that was still finding edge cases in the last few
commits, a fix that fixes another fix in the same range), reach for `next` deliberately.

Apply the following rules based on the optional argument:

### No argument — judge the level, land stable unless it is a major

1. **Judge the required level** from the Step 3 commit categorization:
   - any **Breaking Change** (`!` suffix or `BREAKING CHANGE` in the body) → **major**
   - else any `feat:` → **minor**
   - else → **patch**

2. **Compute the target version:**
   - **From a stable `X.Y.Z`** — bump the judged component: patch → `X.Y.{Z+1}`, minor → `X.{Y+1}.0`.
   - **From a prerelease `X.Y.Z-next.N`** — **promote to its own triple** `X.Y.Z` (drop the suffix).
     The triple already encodes a level relative to the last stable (`1.1.0` after `1.0.0` already
     claims "minor"), so fixes *and* feats landed during the soak need no further bump:
     `1.1.0-next.1` + fixes → `1.1.0`; `1.1.0-next.1` + a `feat:` → still `1.1.0`. Escalate the
     triple ONLY for a breaking change (→ `{X+1}.0.0`), which routes to the ask in 4.

3. **patch or minor → land it STABLE, fully autonomously.** No prompt, no prerelease step, no
   waiting. Examples:
   - `1.0.0` + fixes only → `1.0.1`
   - `1.0.0` + a `feat:` → `1.1.0`
   - `1.1.0-next.1` + anything non-breaking since the tag → `1.1.0`

4. **major → STOP and ASK.** Present the breaking commits and wait for the user to pick:
   - **stable major now** → `{X+1}.0.0`, straight to `latest`
   - **prerelease first** → `{X+1}.0.0-next.1`, soak on `next`, promote later with `stable`

   This is the ONE place the default autonomous path pauses, and it is deliberate. A major asserts
   "this breaks existing projects"; `latest` is what a bare `npm i`, `brew install`, and `curl | sh`
   resolve to; and a published version is immutable on npm. Do not guess the channel here — ask.

### `next` argument — force a prerelease (the soak escape)

The primary way to ask for dogfooding, since the no-argument path no longer produces prereleases.

- **From a stable `X.Y.Z`** — start a prerelease at the judged level (rule 1 above):
  patch → `X.Y.{Z+1}-next.1`, minor → `X.{Y+1}.0-next.1`, major → `{X+1}.0.0-next.1`.
  - Example: `1.0.0` + a `feat:` → `1.1.0-next.1`
- **From a prerelease `X.Y.Z-next.N`** — continue the existing line: `X.Y.Z-next.{N+1}`.
  - Example: `1.1.0-next.1` → `1.1.0-next.2`
  - This case used to be an error (no-argument owned line-continuation). Now that no-argument
    **promotes** a prerelease to stable, `next` is the only way to extend a soak — so it must work
    here, and it is unambiguous.
- To restart a prerelease on a *different* triple, pass `major` / `minor` / `patch` explicitly.

### `major` argument

- Bump major, reset minor+patch, start prerelease: `{X+1}.0.0-next.1`
- Example: `0.1.0-next.5` → `1.0.0-next.1`, `0.1.0` → `1.0.0-next.1`

### `minor` argument

- Bump minor, reset patch, start prerelease: `X.{Y+1}.0-next.1`
- Example: `0.1.0-next.5` → `0.2.0-next.1`, `0.1.0` → `0.2.0-next.1`

### `patch` argument

- Bump patch, start prerelease: `X.Y.{Z+1}-next.1`
- Example: `0.1.0-next.5` → `0.1.1-next.1`, `0.1.0` → `0.1.1-next.1`

### `stable` argument (no level) — promote the current prerelease

- Strip the `-next.N` suffix from the current prerelease.
- Requires current version to be a `-next.N` prerelease. If it is stable already, stop with an
  error and point the user at `stable <level>` below — that is the form for stable → stable.
- Example: `0.1.0-next.5` → `0.1.0`

### `stable <level>` argument — land a stable release directly

`stable major`, `stable minor`, or `stable patch`. Bumps that component of the current version's
release triple, discards any `-next.N` suffix, and lands the result **stable** — no intermediate
prerelease, one release cycle instead of two.

- `stable patch`: `X.Y.{Z+1}` — Example: `1.0.0` → `1.0.1`
- `stable minor`: `X.{Y+1}.0` — Example: `1.0.0` → `1.1.0`
- `stable major`: `{X+1}.0.0` — Example: `1.0.0` → `2.0.0`

Works from a prerelease too, computing from the release triple and dropping the suffix — this is
the form that produces a **first stable release**: `0.1.0-next.99` + `stable major` → `1.0.0`.
(Before this rule existed, cutting v1.0.0 required driving Steps 4–11 by hand, because no argument
could produce a stable major.)

**Post-1.0 semver applies.** Once a stable holds `latest`, the component choice is a compatibility
claim, not a size estimate: `stable patch` asserts no API change, `stable minor` asserts additive
only, and anything that breaks an existing project requires `stable major`. Do NOT default to
`patch` because the diff looks small.

### Validation (all forms)

After computing the proposed version, before any mutation:

- It MUST be strictly greater than the current version under semver precedence (a prerelease sorts
  below its own stable: `1.0.0-next.1` < `1.0.0`). If it is not, stop with an error showing both
  versions — never bump sideways or backwards.
- It MUST NOT already exist as a **published** GitHub Release (Step 8 re-checks this against
  drafts; this is the earlier, cheaper guard). A published version is immutable on npm and can
  never be re-cut.

## Step 3: Analyze Changes and Propose

Find the latest version tag. First fetch remote tags — under the X9 flow, prior releases created their `v*` tag only on GitHub (via the draft `gh release create`), so the most recent tag may be absent from this local checkout. Without the fetch, `git tag -l` picks a stale older tag and the changelog base re-includes already-released commits:

```bash
git fetch --tags origin
git tag -l 'v*' --sort=-v:refname | head -1
```

**Pick the changelog base by what you are releasing:**

- **Normal case** — base = the latest `v*` tag (the command above).
- **Promotion** — the target is stable `X.Y.Z` and the current version is `X.Y.Z-next.N` (the
  no-argument path from a prerelease, or the `stable` argument). Use the latest **stable** tag as
  the base instead:

  ```bash
  git tag -l 'v*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1
  ```

  Match stable tags **positively** (bare `vMAJOR.MINOR.PATCH`) rather than excluding prereleases
  with something like `grep -v -- '-'`: that negative form is not portable — `ugrep`, which shadows
  `grep` on some setups, rejects a bare `-` as a pattern and fails the whole pipeline. The positive
  regex also drops `-beta.` / `-rc.` tags for free.

  A promotion's changelog must describe what changed since the last thing on `latest`, not since the
  last prerelease. Basing it on the prerelease tag would make `v1.1.0`'s page empty even though the
  release carries every commit of the `1.1.0-next.*` line. The breaking-change scan that feeds
  Step 2's level judgment uses this same base — a `feat!:` that landed mid-soak must still escalate
  `1.1.0` to `2.0.0`.

Analyze commits since the chosen base:

```bash
git log <base-tag>..HEAD --oneline
```

**Zero-commit guard.** If that command returns nothing, the base tag is already at HEAD and there
is nothing to release — **STOP with an error**, naming the tag and showing that the SHAs match
(`git rev-parse <base-tag> HEAD`). An empty commit range is never what "cut a release" meant, and on
a `stable` form the result would land on `latest` immutably. This guard is **not** waived by the
default autonomous mode and is independent of `--confirm`; autonomy removes confirmation prompts,
it does not authorize publishing an empty release. (This is a live hazard, not a hypothetical: right
after a release lands, `v<just-released>` IS HEAD, so an immediate re-invocation hits exactly this.)

Note the base selection above is what makes a promotion work: promoting with no *new* commits since
the prerelease is legitimate — you are changing the channel, not the content — so the guard measures
against the last **stable** and correctly lets it through. Only a range that is empty since the last
stable is genuinely nothing to release.

Categorize each commit by its conventional-commit prefix:

- **Breaking Changes**: commits with `!` suffix (e.g. `feat!:`) or `BREAKING CHANGE` in body
- **Features**: `feat:` prefix
- **Bug Fixes**: `fix:` prefix
- **Other Changes**: everything else (`docs:`, `chore:`, `refactor:`, `ci:`, `test:`, `style:`, `perf:`, etc.)

Then classify every user-facing commit and diff into package lanes by ownership:

- **zfb**: the Rust engine/CLI, `@takazudo/zfb`, and all native carrier packaging.
- **zfb-runtime**: browser/runtime package behavior and API.
- **zfb-adapter-cloudflare**: Cloudflare adapter behavior and API.
- **create-zfb**: generator CLI and generated-project behavior.
- **zfb-md-wasm**: the MD/WASM package, entries, API, artifacts, and package behavior.

A change that affects multiple packages MUST appear in every affected lane. Do not put repo-only
docs, tests, CI, or maintenance changes with no package-facing effect in any lane. Every lane still
gets a page for the lockstep version; when a lane has no package-specific change, preserve its date
and use exactly `- No package-specific changes.` Never copy another package's narrative into an
unchanged lane.

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

Only show sections that have entries.

This categorization is also what feeds Step 2's no-argument level judgment (breaking → major,
else `feat:` → minor, else patch), so do it before finalizing the proposed version.

- **Default (autonomous)**: the printout is for visibility only — proceed straight to Step 4 without waiting.
- **Exception — no-argument MAJOR**: if the range contains a breaking change and no explicit version
  argument was given, do NOT proceed. Show the breaking commits and ask the user to choose
  **stable `{X+1}.0.0`** or **prerelease `{X+1}.0.0-next.1`**, per
  [Step 2's no-argument rule](#no-argument--judge-the-level-land-stable-unless-it-is-a-major).
  Resume full autonomy once they answer. (An explicit `major` / `stable major` / `next` argument
  already states the intent — no ask.)
- **With `--confirm`**: **wait for explicit user confirmation before proceeding.** If the trigger was a loose phrase, restate the proposed bump plainly so the user can catch a wrong version strategy before anything is written.

## Step 4: Bump + Sync + Package Changelog MDX

### 4a. Update packages/zfb/package.json

Update the `version` field in `packages/zfb/package.json` to the confirmed new version (without the `v` prefix). Do NOT touch the workspace root `package.json`.

### 4b. Propagate to lockstep packages

```bash
node scripts/sync-platform-versions.mjs
```

This propagates the new version to all lockstep packages and updates `optionalDependencies` in `packages/zfb/package.json`.

### 4c. Regenerate lockfile

```bash
pnpm install --lockfile-only
```

This regenerates `pnpm-lock.yaml` (so CI's `pnpm install --frozen-lockfile` succeeds) **without touching `node_modules`**. Use `--lockfile-only` rather than a plain `pnpm install`: bumping the workspace versions makes pnpm consider `node_modules` stale, so a full install wants to **purge and relink it** — and under a non-interactive shell (no TTY) that aborts with `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`. For a version-only bump the lockfile diff is just the `specifier: workspace:<old>` → `<new>` lines (a registry-sourced `deprecated:` annotation on an unrelated transitive dep may also appear — benign; keep it, a fresh resolve produces it too).

If a later step needs a consistent `node_modules` (the commit hook's `pnpm exec prettier`, or the Step 5 tests), do one full sync with the purge auto-confirmed: `CI=1 pnpm install`.

**Lockfile drift heuristic** — before staging, run (the goal is to surface added/removed lines that are NOT simple two-space-indented entries; `grep -E` cannot do a negative lookahead, so use `grep -P` where available, else the awk fallback):

```bash
# PCRE (GNU grep -P / ripgrep): show +/- lines that are NOT two-space-indented
git diff pnpm-lock.yaml | grep -P '^[+-](?!  )' | head -20
# Portable fallback (BSD/macOS grep lacks -P):
# git diff pnpm-lock.yaml | awk '/^[+-]/ && !/^[+-]  / { print } ' | head -20
```

If you see non-version-related changes (structural changes, unexpected lines), stop and surface the diff to the user before proceeding.

### 4d. Write five package changelog MDX pages

Create exactly these five English pages (there are no Japanese mirrors because the changelog is
default-locale-only):

```text
docs/src/content/docs/changelog/zfb/v<version>.mdx
docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx
docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx
docs/src/content/docs/changelog/create-zfb/v<version>.mdx
docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx
```

Use this shape for each page:

```mdx
---
title: 'v<version>'
sidebar_position: <computed>
# Include only for a lane's first-ever release page:
pagination_next: null
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

- Only include sections that have entries for that package. If there are none, the entire body
  after `Released:` is exactly a blank line followed by `- No package-specific changes.`
- Use today's date for `Released`.
- Each entry: commit subject followed by the short hash in parentheses.
- Duplicate cross-package changes into every affected page.
- Omit repo-only docs/tests/CI/maintenance changes with no package-facing effect.
- Compute `sidebar_position` independently in each package directory. Scan only that lane's
  non-index `v*.mdx` pages, take its maximum, and add one. Run all five scans — never derive one
  lane's position from another or scan the retired changelog root:

  ```bash
  ZFB_POSITION=$(find docs/src/content/docs/changelog/zfb -maxdepth 1 -type f -name 'v*.mdx' -exec grep -h '^sidebar_position:' {} + | awk 'BEGIN { max = 0 } $2 > max { max = $2 } END { print max + 1 }')
  ZFB_RUNTIME_POSITION=$(find docs/src/content/docs/changelog/zfb-runtime -maxdepth 1 -type f -name 'v*.mdx' -exec grep -h '^sidebar_position:' {} + | awk 'BEGIN { max = 0 } $2 > max { max = $2 } END { print max + 1 }')
  ZFB_ADAPTER_CLOUDFLARE_POSITION=$(find docs/src/content/docs/changelog/zfb-adapter-cloudflare -maxdepth 1 -type f -name 'v*.mdx' -exec grep -h '^sidebar_position:' {} + | awk 'BEGIN { max = 0 } $2 > max { max = $2 } END { print max + 1 }')
  CREATE_ZFB_POSITION=$(find docs/src/content/docs/changelog/create-zfb -maxdepth 1 -type f -name 'v*.mdx' -exec grep -h '^sidebar_position:' {} + | awk 'BEGIN { max = 0 } $2 > max { max = $2 } END { print max + 1 }')
  ZFB_MD_WASM_POSITION=$(find docs/src/content/docs/changelog/zfb-md-wasm -maxdepth 1 -type f -name 'v*.mdx' -exec grep -h '^sidebar_position:' {} + | awk 'BEGIN { max = 0 } $2 > max { max = $2 } END { print max + 1 }')
  ```

  The migrated `zfb` lane continues after its historical maximum. A lane with no existing version
  pages gets position 1; include `pagination_next: null` in that first page's frontmatter so its
  previous/next traversal cannot cross into another package lane. Omit that key on later pages.
  `index.mdx` is excluded by each `v*.mdx` scan.

  - This replaces the retired encoded-semver mega-number formula
    (`MAJOR*10000000 + MINOR*100000 + PATCH*1000 + …`, values like `20700999`). All 110 pre-existing
    pages were renumbered to plain 1..N in semver order on 2026-08-18 (zudo-doc's own changelog uses
    the same plain-increment style). Do NOT resurrect the mega-number formula: a mega-number page
    sorts above every incremental page and would pin itself to the top of the sidebar forever.
  - Edge case — releasing on an older line (e.g. a `2.6.x` patch after `2.7.0` exists): plain
    max+1 would place it above `2.7.0` in the sidebar. This has never happened in this repo
    (releases are strictly forward); if it ever does, renumber the affected tail by hand so
    position order matches semver order, and say so in the release report.

## Step 5: Build + Test (focused)

```bash
pnpm --filter @takazudo/zfb test && \
  cargo test --package zfb && \
  pnpm --filter docs check && \
  pnpm --filter docs build && \
  pnpm --filter docs check:html
```

The docs build's `--strict-broken` flag is supplied by the `docs` package's `build` script. Run all
five commands before the direct release push so type/content errors, strict broken links, and
malformed emitted HTML cannot be published. If anything fails, stop and tell the user. Do not
proceed.

If you used `--lockfile-only` in 4c (so `node_modules` is still "stale" per pnpm), the TS test's pre-run deps check will try to auto-install and hit the same no-TTY purge abort. Either run `CI=1 pnpm install` once first, or skip the check for this run: `pnpm --config.verify-deps-before-run=false --filter @takazudo/zfb test` (the bump changes only internal version numbers, not external deps, so the existing `node_modules` is valid for the test — and CI re-validates with a clean install at Step 7 regardless). The `cargo test` leg is unaffected.

If this release touches `packages/zfb-runtime` router code, also run `pnpm test:webkit-back` (T4 local-heavy, Mac only — not covered by Step 7's CI wait; `pnpm test:router-chromium` already runs in CI via `router-chromium.yml`).

Note: the Rust CLI binary is built by `.github/workflows/release.yml` — do not attempt to build it here.

## Step 6: Atomic Commit + Push

Stage and commit all bumped files atomically in a **single commit**:

```bash
git add packages/*/package.json crates/zfb-md-wasm/npm/package.json pnpm-lock.yaml crates/zfb/src/commands/new.rs \
  docs/src/content/docs/changelog/zfb/v<version>.mdx \
  docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx \
  docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx \
  docs/src/content/docs/changelog/create-zfb/v<version>.mdx \
  docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx
git commit -m "chore(release): bump to v<version>"
git push origin main
```

Note: `crates/zfb/src/commands/new.rs` no longer changes on a version bump — the scaffold dependency pin is self-syncing (derived at compile time from the binary's own release version; see `workspace_dep_placeholder()` and issue #503). It is kept in the `git add` line purely defensively; the add is a harmless no-op when the file is unchanged.

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

If it exists:

- **Default (autonomous)**:
  - Existing release is a **draft** (`gh release view v<version> --json isDraft --jq '.isDraft'` is `true`) → delete and recreate: `gh release delete v<version> --yes` (no `--cleanup-tag` — a never-published draft has no tag ref), then proceed to Step 9. Report what was deleted.
  - Existing release is **published** → STOP with an error. The version is already live; never delete a published Release. Re-run `/l-make-release` so Step 2 bumps past it.
- **With `--confirm`**: present the user with three options and wait for their choice:
  - **Reuse**: skip the `gh release create` step and proceed to the notify message.
  - **Delete and recreate**: `gh release delete v<version> --yes` then re-create (only for drafts — never delete a published Release).
  - **Abort**: stop.

This check is scoped to the **target** version. A draft for a *different* (earlier, superseded) version is an orphan from an abandoned run — that case is caught by Step 1's draft scan and cleaned up via ["Cancelling a release / cleaning up an orphaned draft"](#cancelling-a-release--cleaning-up-an-orphaned-draft).

Also verify that the most-recent commit on `main` matches the version in all five MDX pages (each
`Released:` date and each filename `v<version>.mdx` should align with the current HEAD). If any page
is missing or mismatched, surface it and recommend rollback before proceeding.

## Step 9: Pre-create Draft GH Release

```bash
ZFB_NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/zfb/v<version>.mdx)
ZFB_RUNTIME_NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx)
ZFB_ADAPTER_CLOUDFLARE_NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx)
CREATE_ZFB_NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/create-zfb/v<version>.mdx)
ZFB_MD_WASM_NOTES=$(sed -n '/^Released:/,$ p' docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx)
RELEASE_NOTES=$(printf '%s\n\n%s\n\n%s\n\n%s\n\n%s\n\n%s\n\n%s\n\n%s\n\n%s\n\n%s' \
  '## @takazudo/zfb' "$ZFB_NOTES" \
  '## @takazudo/zfb-runtime' "$ZFB_RUNTIME_NOTES" \
  '## @takazudo/zfb-adapter-cloudflare' "$ZFB_ADAPTER_CLOUDFLARE_NOTES" \
  '## create-zfb' "$CREATE_ZFB_NOTES" \
  '## @takazudo/zfb-md-wasm' "$ZFB_MD_WASM_NOTES")
PRERELEASE_FLAG=$([[ "<version>" =~ -next\.|-beta\.|-rc\. ]] && echo "--prerelease" || echo "")
gh release create v<version> --target <bump-sha> --title "v<version>" --notes "$RELEASE_NOTES" --draft $PRERELEASE_FLAG
```

Keep these five extractions independent: never reuse one lane's variable as another package's body.
This remains one GitHub Release with the existing tag, binary assets, npm publication order, and
Homebrew flow; only its notes are assembled from the five package sources.

The tag is created remotely as a draft. The `release: published` webhook event does NOT fire on draft creation (by design).

## Step 10: Build the macOS x86_64 Binary Locally (default)

GitHub's `macos-15-intel` runners are slow and frequently queue-starved, so this skill builds the Mac binary **locally by default** and pre-uploads it to the draft Release. `release.yml`'s detect-mac-local job then skips the `macos-15-intel` leg at publish time.

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

Both `zfb-<version>-x86_64-apple-darwin.tar.gz` and its `.sha256` companion must appear. If either is missing, stop and surface what was found vs. expected. The draft Release already exists at this point — if the user chooses to abandon this release rather than retry the upload, tear it down via ["Cancelling a release / cleaning up an orphaned draft"](#cancelling-a-release--cleaning-up-an-orphaned-draft) so the next run starts clean.

### If NOT `Darwin`

Do not attempt the build (the cross-target needs an Apple host). **Print this note**, then continue to Step 11. (Default: Step 11 publishes anyway and `release.yml`'s macos-15-intel leg builds the Mac binary on CI — slower but autonomous. With `--confirm`: the user can run `/l-make-mac-release-binary` on a Mac before publishing manually.)

```
⚠️  Not a macOS host (uname = <result>). Skipping the local Mac build.
    To build the Mac binary locally (recommended — saves the slow macos-15-intel CI leg),
    run on a Mac before publishing:

      /l-make-mac-release-binary v<version>

    Otherwise, publishing the draft will let CI build the macos-15-intel leg (slower).
```

## Step 11: Publish + Watch (default) / Notify + STOP (`--confirm`)

The Homebrew gating and the prerelease dual-tag note below apply to BOTH paths.

### Default path (autonomous): publish the Release and watch release.yml

Do NOT ask "publish?", "go?", or wait for any signal — publish immediately:

1. **Publish**:

   ```bash
   gh release edit v<version> --draft=false
   ```

2. **Find the triggered `release.yml` run** (it fires on `release: published`; allow a few seconds for it to appear):

   ```bash
   gh run list --workflow release.yml --limit 3 --json databaseId,displayTitle,status
   ```

3. **Watch the run to completion** with a background poll (same pattern as `/watch-ci` — `gh run view <id> --json status,conclusion` every 30s until `completed`; do NOT poll in the foreground). The run builds the remaining platform archives (linux + windows; the macos-15-intel leg is skipped when the Mac archive was pre-uploaded in Step 10, built on CI otherwise) and publishes all 10 npm packages.
4. **On success — update Homebrew (stable only), then report.**

   **a. Homebrew.** If `<version>` is **stable** (no `-next.` / `-beta.` / `-rc.`), run it now — do
   not ask, and do not defer it to the user:

   ```bash
   ./scripts/update-homebrew-formula.sh v<version> --push
   ```

   Run it only **after** the `release.yml` watch reports success: the script fetches every
   platform's `.sha256` from the Release and 404s if the upload job has not finished. It is
   idempotent — re-running for the same version rewrites `Formula/zfb.rb` to the same content and
   commits nothing new — so a retry after a transient network failure is safe.

   Confirm the tap actually moved, rather than trusting the exit code alone:

   ```bash
   TAP="${ZFB_TAP_PATH:-${HOME}/repos/zp/homebrew-tap}"
   git -C "$TAP" log -1 --oneline
   grep -m1 'version' "$TAP/Formula/zfb.rb"
   ```

   **If it fails, the release is still a success** — npm and the GH Release are already live and
   immutable. Report the brew failure separately with the command to retry by hand; do NOT unpublish
   anything, and do NOT retry more than once. The usual causes are a push credential problem on the
   tap remote or a `.sha256` not yet uploaded.

   For **prereleases**, skip this entirely — brew tracks the stable channel. Testers use
   `npm i -g @takazudo/zfb@next` or the curl installer with `ZFB_VERSION=latest-prerelease`.

   **b. Report.** Release URL, and confirm npm landed with `npm view @takazudo/zfb dist-tags`. For a
   stable release, state the tap commit so the Homebrew half is verifiable at a glance.
5. **On failure**: fetch the failed logs (`gh run view <id> --log-failed`) and report. If the failure is clearly transient (network flake, runner eviction), retry once with `gh run rerun <id> --failed`. Otherwise surface to the user with the failure summary — do NOT unpublish or delete the Release, and do NOT retry more than once.

### `--confirm` path: notify + STOP

Print the message below **verbatim** (substitute the actual version string for `<version>`), picking the block that matches whether the Mac archive was uploaded in Step 10. Do not paraphrase command strings or URLs.

The Homebrew step is gated to **stable** releases (it tracks the stable channel, like npm `latest`). If `<version>` is a prerelease (`-next.` / `-beta.` / `-rc.`), do NOT run `update-homebrew-formula.sh` — direct prerelease testers to `npm i -g @takazudo/zfb@next` or the curl installer's `ZFB_VERSION=latest-prerelease`.

**Note — prerelease dual-tag (RESOLVED as of v1.0.0, 2026-07-31)**: while
`@takazudo/zfb dist-tags.latest` was empty or was itself a prerelease (contains `"-"`),
`release.yml` advanced **both** `next` and `latest` on every `*-next.*` publish, so `npm i -g @takazudo/zfb`
(no tag) followed prereleases. **v1.0.0 now holds `latest`, so that gate is self-disabled and
prereleases no longer touch `latest`** — this is history, not current behavior. Do not expect a
`-next.N` publish to move `latest`. See RELEASE_DAY_CHECKLIST.md "Prerelease dual-tag policy" for
the manual remediation commands if the workflow's `dist-tag add` retries ever exhaust.

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
skip the slow macos-15-intel leg, and publish all 10 packages.

After publishing, WAIT for the Release workflow run to finish — it builds and uploads the
remaining platform archives (linux + windows) and their .sha256 files, then publishes the
npm packages:

  gh run watch

If this is a STABLE release, update Homebrew once the Release run above succeeds (the script
fetches every platform's .sha256 from the Release and 404s if it has not finished). SKIP this
for prereleases — brew tracks the stable channel; testers use `npm i -g @takazudo/zfb@next` or the curl
installer with ZFB_VERSION=latest-prerelease:

  ./scripts/update-homebrew-formula.sh v<version> --push

(Homebrew is manual on THIS path only, because --confirm stops before publishing. The default
path runs the tap push itself. See RELEASE_DAY_CHECKLIST.md for the Homebrew flow.)
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

Option B: publish now and let CI build the macos-15-intel leg (slower)

    gh release edit v<version> --draft=false
    # or via the web UI

Either way, release.yml auto-detects whether the Mac archive is on the Release at
publish time. If present → skip macos-15-intel (fast). If absent → build on CI.

After publishing, WAIT for the Release workflow run to finish — it builds and uploads the
remaining platform archives (linux + windows, and the macos-15-intel leg under Option B) and their
.sha256 files, then publishes the npm packages:

  gh run watch

If this is a STABLE release, update Homebrew once the Release run above succeeds (the script
fetches every platform's .sha256 from the Release and 404s if it has not finished). SKIP this
for prereleases — brew tracks the stable channel; testers use `npm i -g @takazudo/zfb@next` or the curl
installer with ZFB_VERSION=latest-prerelease:

  ./scripts/update-homebrew-formula.sh v<version> --push
============================================================
````

Then **STOP**. On the `--confirm` path the skill does NOT publish the draft — the user does. (On the default path, publishing already happened above and the skill ends after the release.yml watch + final report.)

## Cancelling a release / cleaning up an orphaned draft

A draft GH Release is created at Step 9 and may have a Mac binary uploaded at Step 10, **before** the skill stops (Step 11) for the user to publish. If the release is then abandoned — a problem is found, or the user keeps developing without ever publishing — that draft is left **orphaned** on GitHub. Drafts never fire the `release: published` webhook, so they are harmless to CI, but they accumulate and confuse the next run's partial-state detection.

Invoke this cleanup when:

- The user runs `/l-make-release cancel` (or asks to "cancel/abort the release", "remove the draft"), or
- A problem is found mid-release after the draft was created (e.g. a Step 10 failure the user does not want to retry), or
- Step 1's orphaned-draft scan surfaces a leftover draft the user wants gone.

### Identify the draft(s)

If no version is implied by context, list all drafts and let the user pick:

```bash
gh release list --json name,isDraft,tagName --jq '.[] | select(.isDraft) | .tagName'
```

- **None** → nothing to cancel; report that and stop.
- **Exactly one** → propose deleting it (state the tag), then act once the user has authorized cleanup.
- **Multiple** → list them and ask which to delete.

### What to remove

1. **Confirm it is a draft, then delete it** (deletion also removes its uploaded assets — the Mac archive + `.sha256`):

   ```bash
   gh release view v<version> --json isDraft --jq '.isDraft'   # must be true
   gh release delete v<version> --yes
   ```

   Do **NOT** pass `--cleanup-tag` for a never-published draft. GitHub stores the intended tag name but has not created the git ref, so `--cleanup-tag` returns `HTTP 422: Reference does not exist` and a non-zero exit *after* the release is already gone — noisy and misleading (the release deletion itself succeeded). A draft has no tag to clean up. And **NEVER** delete a **published** Release here — that would remove a live tag consumers may depend on.

2. **Decide whether to undo the bump commit.** Check where the bump sits relative to HEAD:

   ```bash
   git rev-list --count <bump-sha>..HEAD
   ```

   - **`0` — the bump is still HEAD** (created this run, nothing built on top): revert it. The Step 6 commit is atomic, so one revert undoes `package.json` + the lockfile **and** removes all five new package MDX pages together:

     ```bash
     git revert --no-edit <bump-sha>
     git push origin main
     ```

   - **`>0` — the bump is buried under later commits** (the common "abandoned then kept developing" case): do **NOT** revert or rewrite history. The stale version number in `packages/zfb/package.json` is harmless — the next release simply bumps from it and supersedes the abandoned version (e.g. an abandoned `…-next.11` is superseded by the next `…-next.12`). Delete **only** the orphaned draft Release (step 1) and stop.

### After cleanup

Report what was deleted (tag + assets) and which case applied (reverted bump vs. left buried). The repo is now clean for a fresh `/l-make-release` run when the user is ready.

## Failure Recovery

### pnpm-lock.yaml drift

Run `git diff pnpm-lock.yaml | grep -E '^[+-](?!  )' | head -20` before staging. If non-version-line structural changes appear, stop and surface the diff. Fix the lockfile manually before re-running.

### Build or test failure (Step 5)

Stop and report the failure. Do not proceed with the commit. Fix the issue and re-run the skill.

### CI fails on bump commit (Step 7)

Fix the issue, commit the fix, push, then re-invoke `/watch-ci`. Do not proceed to the draft Release until CI is green.

### Release workflow is missing or fails after the Release is published

Never unpublish/delete the published Release, move its tag, or publish npm packages directly. If the cause is in `release.yml`, fix it on `main`, commit + push the fix, and wait for main CI to pass. Then run the fixed workflow definition from `main` against the existing published tag:

```bash
gh workflow run release.yml --ref main \
  -f dry_run=false \
  -f skip_macos_x64=false \
  -f release_tag=v<version>
```

Do not select `v<version>` as `--ref`: GitHub would load the old workflow definition from that tag. The workflow's `release-context` job verifies the published Release, tag commit, and package version, then pins every source checkout to that exact commit. Because GitHub OIDC still identifies the `main` workflow commit, this recovery path publishes all packages without npm provenance rather than attaching a misleading source attestation. Watch this recovery run to completion using the same Step 11 procedure. Once it succeeds, perform the normal stable Homebrew update exactly once.

### Existing draft Release for the version (Step 8)

Default (autonomous): draft → delete and recreate; published → stop with an error (never delete a published Release). With `--confirm`: prompt reuse / delete-and-recreate / abort and wait for user choice before acting.

### Mismatched package MDX + commit (Step 8)

Surface the mismatch clearly. Recommend rolling back (see "Rolling back the bump" below), then re-run `/l-make-release`. Wait for user decision.

### Orphaned / abandoned draft Release

A draft created in a prior run that was never published — the most common leftover. Detected by Step 1's draft scan (or `gh release list --json name,isDraft,tagName`). Clean it up via ["Cancelling a release / cleaning up an orphaned draft"](#cancelling-a-release--cleaning-up-an-orphaned-draft): delete the draft (and its assets) with `gh release delete v<version> --yes` (no `--cleanup-tag` — a never-published draft has no tag ref), and do **not** rewrite history if the bump commit is already buried under later commits.

### Rolling back the bump

If a draft Release was already created for this version, delete it first (see ["Cancelling a release / cleaning up an orphaned draft"](#cancelling-a-release--cleaning-up-an-orphaned-draft)). Then, **only if the bump commit is still HEAD** (`git rev-list --count <bump-sha>..HEAD` is `0`):

```bash
git revert --no-edit <bump-sha>
git push origin main
```

The atomic Step 6 commit means one revert undoes `package.json`, the lockfile, **and** all five new package MDX pages together — separate `rm` commands are not needed. If the bump is buried under later commits, do NOT revert; leave the version and let the next release supersede it. Then re-run `/l-make-release` from the start.
