# Releasing zfb

This is the **one canonical release procedure** for `@takazudo/zfb` and its ten lockstep packages.
It is tool-neutral: a human, a local Claude Code session (`/l-make-release`) or a Codex Cloud
session (`.agents/skills/zfb-release`) follows the same steps from a fresh checkout. The skills
are thin entry points into this file; when they disagree with it, this file wins.

Related references, not substitutes:

- [`CLAUDE.md`](CLAUDE.md) — the npm dist-tag rule, the five-lane changelog contract, the
  Rosetta and macOS-on-the-owner's-Mac policy and the development strategy. Codex does not load
  `CLAUDE.md` automatically; read its "npm dist-tags" and "Five-lane release changelog contract"
  sections before a release.
- [`scripts/darwin-artifact-contract.md`](scripts/darwin-artifact-contract.md) — the Darwin
  release-input contract, receipt schema and admission rules.
- [`RELEASE_DAY_CHECKLIST.md`](RELEASE_DAY_CHECKLIST.md) — channel policy, asset layout and
  preserved incident history.
- [`research/npm-oidc-cutover.md`](research/npm-oidc-cutover.md) — the owner's npm trusted
  publisher setup, live-proof checklist and credential retirement runbook.

## Authority and stop conditions

A release is a production action: it pushes to `main`, creates a tag, publishes eleven
immutable npm versions and moves public channels. Only the owner authorizes one, per release.
Development integration, a merged PR or a green CI run never implies release authorization.

An agent following this file **stops and reports** (never works around) when:

- the release needs Darwin inputs that are missing or fail admission (only the owner's Mac can
  produce them — see [Phase M](#phase-m-darwin-inputs-on-the-owners-mac));
- the provenance preflight reports a trust downgrade that the owner has not acknowledged (see
  [Provenance contract](#provenance-contract-and-the-downgrade-blocker));
- a recovery dispatch would need `allow_provenance_downgrade=true`;
- a step would create or edit `release/provenance-policy.json` (owner-authored only);
- a no-argument version judgment lands on a **major** (version-strategy question);
- GitHub capability is missing for the next step (push, Release write, workflow dispatch);
- `scripts/release/release-status.mjs` recommends `blocked:<reason>`.

Never: delete or edit a **published** Release's tag, publish npm packages by hand, use
`gh release upload --clobber`, create a second Release for a version, add a hosted macOS runner,
invent or copy qualification evidence, weaken a gate, or treat pending/cancelled/skipped/
pass-on-retry as passed.

## Environments and phases

| Phase | Where | Who | What |
| --- | --- | --- | --- |
| **A — any environment** | owner's Mac, any Linux host, or Codex Cloud | human or agent | version, changelog, measurements, bump commit, CI, draft Release, admission, publish, watch, verify, promote, Homebrew |
| **M — Mac only** | the owner's Apple Silicon Mac | owner (or the owner's local agent session) | build and qualify both Darwin targets with `scripts/build-darwin-local.sh` and upload them to the draft Release |

There is **no hosted macOS runner** in the release. Both Darwin platform packages are built on
the owner's Mac from the exact release source and admitted in CI from the draft Release's assets.
`x86_64-apple-darwin` is qualified under Rosetta 2 and is labelled **Rosetta-translated** — never
"native Intel"; it is functional evidence only (no performance, timing or CPU-feature claim).

A Cloud or Linux session can only **consume** Darwin inputs that the Mac already uploaded. When
they are missing, the release is blocked: hand the owner the
[paste-ready Mac prompt](#paste-ready-mac-prompt) and stop. A Mac-only test never counts as Cloud
acceptance, and a Cloud run never creates Mac evidence.

**Moving tag creation to Actions does not move the whole release to the Cloud.** A session that
cannot push tags, such as Claude Code Cloud, can start publication through the
[Actions route](#a12-actions-route-release-coordinator). Cloud still cannot do these steps:

| Step | Who can do it |
| --- | --- |
| Phase M (Darwin build and receipts) | Only the owner's Mac. |
| A provenance-downgrade acknowledgement | Only the owner. |
| The A8 push to `main` | Needs the ruleset's admin bypass. A session without it prepares the bump as a PR the owner merges. |
| The A15 Homebrew push | Needs tap credentials. |

The owner's Mac and the owner still finish every release that needs these steps.

### Heavy local commands

Some steps (Rust tests, the Darwin build) are heavy. Run them through the machine-wide guard when
one exists, mirroring `scripts/run-b4push.sh` and `scripts/build-darwin-local.sh`:

```sh
guard="${HEAVY_GUARD:-}"
for c in "$HOME/.claude/scripts/heavy-guard.sh" "$HOME/.codex/scripts/heavy-guard.sh"; do
  [ -z "$guard" ] && [ -x "$c" ] && guard="$c"
done
${guard:+"$guard" --} <command>
```

The guard is optional. A Cloud host without one runs the command directly; no personal path is
required. Exit 75 from the guard means "not run" (contention), never a test failure.

### GitHub capability (check first)

Every session starts by proving it can do the next step instead of discovering it mid-release:

```sh
node scripts/release/release-status.mjs --tag v<version> --probe-capabilities
```

It reports `gh auth status`, the repository permission (`gh api repos/{owner}/{repo}` →
`.permissions`), whether Actions workflows are readable, and whether `release-coordinator.yml`
exists. It never dispatches anything.

An unauthenticated `gh` in a Cloud session is not a reason to push a tag from a local checkout.
Use the GitHub capability the session does have, such as a GitHub MCP server that can run a
workflow and read runs, for the [Actions route](#a12-actions-route-release-coordinator). If the
session has none, report the exact owner action: the `gh workflow run release-coordinator.yml`
command, or the Actions UI's "Run workflow" with the same inputs. Phase A
needs push to `main` (the release bump goes straight to `main` through the ruleset's admin bypass,
which exists only for this authorized procedure), Release write and `actions: write` for
dispatches. Missing capability is a stop, not a reason to change credentials or protection.

## Identities to record

Record these in the release report (and keep JSON evidence outside the checkout):

- **Source SHA** — the release bump commit. The draft Release targets it, the tag peels to it,
  Darwin receipts and admission bind to it and its tree, and `release.yml` builds it. It is
  frozen once pushed: later commits on `main` never become part of this release.
- **Source tree** — `git rev-parse <source-sha>^{tree}`.
- **Tested checkout** — `git rev-parse HEAD` and `HEAD^{tree}` (plus `git status --porcelain`)
  where the focused checks of [A7](#a7-focused-checks) ran. Its tree must equal the source tree;
  if the commit was amended or rebased afterwards, the local results describe something else.
- **Workflow/control SHA** — the commit whose `release.yml` ran. For the normal
  `release: published` event, and for the coordinator's dispatch **at the tag**, it is the source
  SHA. The coordinator itself runs from `main` (its own control SHA, recorded in its job summary),
  but the `release.yml` it starts does not. For a `workflow_dispatch` recovery or finalize
  run from `main` it is `main`'s head at dispatch, which differs from the source SHA; record both.
- **Run evidence** — every run ID, attempt number, job ID and conclusion, first results and all
  retries; the Darwin receipts' digests and the admission verdict.

## Step 0: where am I? (start and after every interruption)

```sh
node scripts/release/release-status.mjs --tag v<version> [--json]
```

Read-only. It reports the release **source** as `{commit, tree, kind}` (`kind` is `tag` when the
tag ref exists, otherwise `draft-target`, taken from a draft Release's 40-hex `targetCommitish`;
source CI and Darwin admission run against that source), the Release (absent/draft/published,
`targetCommitish`), the expected Release assets (including the ten Darwin inputs) as present,
missing or conflicting, the source commit's required CI state, a Darwin admission of the
downloaded assets, per-package npm state, dist-tags, the latest `release.yml` runs for the tag
(run/attempt IDs and conclusions; runs are matched by their exact name
`Release <tag>[ (finalize only)][ (dry run)]`, so a main-based dispatch is found too), and **one** recommended next action:

| Recommendation | Go to |
| --- | --- |
| `prepare-darwin` | [Phase M](#phase-m-darwin-inputs-on-the-owners-mac) (Cloud: hand off and stop) |
| `publish-draft` | [A11](#a11-admission-and-provenance-preflight) then [A12](#a12-publish-the-draft), or its [Actions route](#a12-actions-route-release-coordinator) when the session cannot push tags |
| `wait-npm-processing` | [npm is still processing a package](#npm-is-still-processing-a-package): stop and report |
| `dispatch-recovery` | [Recovery](#recovery-a-published-release-whose-build-or-publish-failed) |
| `dispatch-finalize` | [Finalize](#finalize-only-dist-tags-digest-asset-verification) |
| `blocked:<reason>` | stop and report the reason |
| `complete` | [A14](#a14-verify-delivery) onward if not yet done, else report |

Before publication the source is `kind: draft-target`; after it, `kind: tag`. Every `blocked:`
reason is a stop. Source-resolution reasons:

| Reason | Meaning |
| --- | --- |
| `blocked:release-missing` | no tag ref and no Release for the version (before A10, expected) |
| `blocked:release-target-not-sha` | the draft's `targetCommitish` is a branch name, not a full SHA; delete it and recreate it with `--target "$SOURCE_SHA"` (A10) |
| `blocked:tag-missing` | a **published** Release without a tag ref; investigate, never retag |

Before a version exists (nothing bumped yet), start at [A1](#a1-preconditions). Resuming is always
idempotent: re-run Step 0, act on its single recommendation, re-run Step 0. Never reconstruct state
from memory or a previous session's notes.

## Phase A: prepare (any environment)

### A1. Preconditions

1. Fresh checkout of `main`, clean tree (`git status --porcelain` empty), `git fetch --tags origin`.
2. `gh` authenticated and capable (see [GitHub capability](#github-capability-check-first)).
3. Node and pnpm per `package.json`; `pnpm install --frozen-lockfile`.
4. At least one `v*` tag exists.
5. Orphaned drafts: `gh release list --json isDraft,tagName --jq '.[] | select(.isDraft) |
   .tagName'`. A draft for a version other than the one being released is an orphan from an
   abandoned run; remove it per [Cancel](#cancel-and-clean-up) and report it. A draft for the
   target version is handled in [A10](#a10-partial-state-and-the-draft-release).

### A2. Version judgment

Read the current version from `packages/zfb/package.json` (the source of truth; the workspace root
stays `0.0.0`). Each rule sets **which component bumps** and **which channel** the result lands on:
`-next.N` publishes to npm `next`; a stable version publishes to `latest`, which `npm i`,
`brew install` and `install.sh` resolve.

**No argument — stable by default.** Judge the level from [A3](#a3-change-analysis-and-lanes):
any breaking change (`!` suffix or `BREAKING CHANGE`) → major; else any `feat:` → minor; else patch.

- From stable `X.Y.Z`: patch → `X.Y.{Z+1}`, minor → `X.{Y+1}.0`; land it stable.
- From prerelease `X.Y.Z-next.N`: promote to `X.Y.Z` (the triple already claims its level); only a
  breaking change escalates to `{X+1}.0.0`.
- **Major → stop and ask** the owner: stable `{X+1}.0.0` now, or `{X+1}.0.0-next.1` soak. `latest`
  is immutable once published. This is the only judgment that pauses.

**Explicit arguments:**

| Argument | Result |
| --- | --- |
| `next` | from stable: `-next.1` at the judged level; from `X.Y.Z-next.N`: `X.Y.Z-next.{N+1}` |
| `major` / `minor` / `patch` | prerelease on that component: `{X+1}.0.0-next.1`, `X.{Y+1}.0-next.1`, `X.Y.{Z+1}-next.1` |
| `stable` | strip `-next.N` from the current prerelease (error if already stable) |
| `stable major\|minor\|patch` | bump that component of the release triple and land it stable |

Post-1.0 semver is a compatibility claim, not a size estimate: anything that breaks an existing
project is a major. Prefix-derived levels measure shape, not risk — choose `next` deliberately when
a batch smells like it needs a soak.

**Validation:** the new version must be strictly greater under semver precedence
(`1.0.0-next.1 < 1.0.0`) and must not already exist as a published Release or npm version.

### A3. Change analysis and lanes

```sh
git fetch --tags origin
git tag -l 'v*' --sort=-v:refname | head -1                                    # normal base
git tag -l 'v*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1  # promotion base
git log <base>..HEAD --oneline
```

- Base is the latest `v*` tag; for a **promotion** to stable it is the latest **stable** tag (match
  stable tags positively; some `grep` replacements reject a bare `-` pattern). The breaking-change
  scan uses the same base.
- **Zero-commit guard:** an empty range is nothing to release — stop and show
  `git rev-parse <base> HEAD`. A promotion with no new commits since the prerelease is legitimate
  because its base is the last stable.
- Categorize by prefix: Breaking Changes, Features (`feat:`), Bug Fixes (`fix:`), Other Changes.

Then classify every user-facing commit and diff into package lanes by ownership:

- **zfb**: the Rust engine/CLI, `@takazudo/zfb`, `@takazudo/zfb-slugify`, and all native carrier packaging.
- **zfb-runtime**: browser/runtime package behavior and API.
- **zfb-adapter-cloudflare**: Cloudflare adapter behavior and API.
- **create-zfb**: generator CLI and generated-project behavior.
- **zfb-md-wasm**: the MD/WASM package, entries, API, artifacts, and package behavior.

A change that affects multiple packages MUST appear in every affected lane. Do not put repo-only
docs, tests, CI, or maintenance changes with no package-facing effect in any lane. Every lane still
gets a page for the lockstep version; when a lane has no package-specific change, preserve its date
and use exactly `- No package-specific changes.` Never copy another package's narrative into an
unchanged lane. This is the [five-lane contract](CLAUDE.md#five-lane-release-changelog-contract).

Print the proposal (current → new, categorized commits) for visibility.

### A4. Version-stamped measurements in CI (before the bump reaches `main`)

Three gates move with the version string, so every release version is measured in CI first:

- **md-wasm** — the four `.wasm` files embed `ZFB_RELEASE_VERSION`; exact sizes can move with the
  stamp. Never measure or refresh `shipped-sizes.json` from a Mac build
  ([`LOCAL-VS-CI-SIZES.md`](crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md)). The rustc version moves
  them too, so health.yml's `wasm-md` job and release.yml's `build-wasm-md` job both compile with
  the exact version in `crates/zfb-md-wasm/rust-toolchain-pin`, not floating `stable` (v6.1.0
  measured on 1.99.1 and built on 1.100.0 six hours later, failing its release run). Changing the
  pin is its own reviewed change and needs a fresh measurement.
- **Linux island size** — `research/v3-island-size/decision-linux-x64.json` pins the package
  version; the release must record it with fresh CI evidence.
- **Wind preview digest** — `docs/public/wind-examples/manifest.json`'s `compiler.sourceSha256`
  covers `crates/**/*.{rs,toml,json,css}`, which the bump changes.

Procedure:

1. Record `main`'s SHA. On a disposable branch from it, apply [A5](#a5-bump-sync-lockfile) exactly
   (version + lockfile bytes the bump commit will carry). Push it and open a **draft** PR to `main`.
2. **md-wasm:** from the `wasm-md (default, default)` job's
   `Enforce four-artifact gzip budgets and report metrics` step, copy all four CI-measured columns
   (`finalWasm`, `gzip9`, `glue`, `glueGzip9`) for default, highlight, render and parse. Record
   every mismatch and gzip warning. A missing metric, failed build or exports assert, or a ceiling
   breach blocks the release until diagnosed.
3. **Island size:** download the `health` job's `island-size-linux-x64` artifact (uploaded even on
   failure) and note the run ID, job ID and probe head SHA. The step is expected to pass (the
   metric normalizes the version token). If it failed, a size moved: stop for a reviewed decision.
4. On the probe branch write the rehearsal commit:
   - set `crates/zfb-md-wasm/shipped-sizes.json` `measuredOnVersion` and the measured columns
     (keep ceilings), and check its `measuredWithRustc` equals `crates/zfb-md-wasm/rust-toolchain-pin`
     and the `md-wasm Rust toolchain:` line in that job's log; run `node scripts/assert-md-wasm-size-docs.mjs --fix`, `pnpm format:mdx`,
     then `node scripts/assert-md-wasm-size-docs.mjs`;
   - run
     `node scripts/rebaseline-island-size-linux.mjs --artifact <dir> --run-id <id> --job-id <id> --head-sha <probe-head> --reason release-bump --label "Release probe PR #<pr> run for v<version>"`.
     It refuses a moved raw/`gzipNormalized` total, a fingerprint, lockfile, version or
     `Cargo.lock` difference — those need their own reviewed transition (`--reason
     fingerprint-helper`, `--reason cargo-producer-lock`) landed first. Run `pnpm exec vp fmt` on
     the changed files;
   - write the new Wind digest from
     `node --input-type=module -e "import {compilerSourceDigest} from './docs/scripts/wind-preview-assets.mjs'; console.log(compilerSourceDigest(process.cwd()))"`
     into the manifest. If `Check workspace wind preview assets` reports any other stale asset,
     the compiler output changed: regenerate and review instead.
   - Push and require a new `wasm-md (default)` run with zero budget errors and green
     `Check workspace wind preview assets` and `Docs gate`. Gzip drift warnings within 64 bytes are
     acceptable; exact-size errors and ceiling breaches are not. The ~40-minute `health` job need
     not be awaited here: [A9](#a9-required-ci-on-the-source-sha) gates the bump commit.
5. If `main` moved past the recorded SHA, redo the probe on the new source. Otherwise carry the
   whole probe diff onto `main`'s working tree (`git diff <recorded-main> <probe-head> | git apply`),
   close the probe PR **unmerged** and delete its branch. The Darwin arm64 island contract is not
   part of this probe.

### A5. Bump, sync, lockfile

Applied first on the probe branch and carried over; afterwards re-run on `main` as a check (no
further diff):

1. Set `version` in `packages/zfb/package.json` (no `v`).
2. `node scripts/sync-platform-versions.mjs` — propagates to all lockstep packages and
   `optionalDependencies`.
3. `pnpm install --lockfile-only` — never a plain install here (a non-TTY purge aborts with
   `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`); use `CI=1 pnpm install` later when a consistent
   `node_modules` is needed.
4. Lockfile drift check: `git diff pnpm-lock.yaml | awk '/^[+-]/ && !/^[+-]  / { print }' | head -20`.
   Anything beyond `workspace:` version lines (a benign registry `deprecated:` annotation aside) is
   a stop.

### A6. Changelog MDX (five lanes)

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
- **Never call shipped artifacts "unchanged" without saying what that covers.** The four
  `zfb-md-wasm` `.wasm` artifacts embed `ZFB_RELEASE_VERSION`, so **every** release moves all four
  SHA-256 digests — including a documentation-only patch whose compiled code is identical and whose
  byte sizes do not move at all. A note saying only "artifacts and their sizes are unchanged" reads
  to a digest-pinning consumer as "nothing to re-verify"; that wording in v2.15.1 nearly caused a
  skipped re-pin (#2885). Say *sizes* when you mean sizes, and say that digests still move.
  Point readers to the versioned GitHub Release asset
  `zfb-md-wasm-<version>-shipped-artifacts.json` and the package export
  `@takazudo/zfb-md-wasm/shipped-artifacts.json` for the four SHA-256 digests.
  Do not write a digest table or refer to a transient CI run as the source.
- Compute `sidebar_position` independently in each package directory as if the target page were
  absent. For each target, call the executable helper with that target path; it scans only that
  lane's non-index `v*.mdx` pages, validates their positions, takes the maximum, and adds one.
  Run all five calls — never derive one lane's position from another or scan the retired changelog
  root:

  ```bash
  ZFB_POSITION=$(node scripts/next-changelog-sidebar-position.mjs docs/src/content/docs/changelog/zfb/v<version>.mdx)
  ZFB_RUNTIME_POSITION=$(node scripts/next-changelog-sidebar-position.mjs docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx)
  ZFB_ADAPTER_CLOUDFLARE_POSITION=$(node scripts/next-changelog-sidebar-position.mjs docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx)
  CREATE_ZFB_POSITION=$(node scripts/next-changelog-sidebar-position.mjs docs/src/content/docs/changelog/create-zfb/v<version>.mdx)
  ZFB_MD_WASM_POSITION=$(node scripts/next-changelog-sidebar-position.mjs docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx)
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

### A7. Focused checks

Run on the working tree that becomes the bump commit, and record the
[tested checkout](#identities-to-record):

```sh
pnpm --filter @takazudo/zfb test
<guard> cargo test --package zfb --lib
pnpm --filter docs check
pnpm --filter docs build          # strict broken-link mode via the package script
pnpm --filter docs check:html
node scripts/assert-md-wasm-size-docs.mjs
```

Any failure stops the release. With a `--lockfile-only` tree, either `CI=1 pnpm install` first or
pass `--config.verify-deps-before-run=false` to the TS test. The Rust leg is deliberately `--lib`:
the version stamp forces every V8-linked integration binary to rebuild, and the full suite runs in
`health.yml` on the exact bump commit at [A9](#a9-required-ci-on-the-source-sha). Run the full suite
locally only when the release changes Rust code CI has not tested on `main`. When the release
touches `packages/zfb-runtime` router code, the owner also runs `pnpm test:webkit-back` on the Mac
(T4, never in CI).

### A8. Commit and push (freezes the source SHA)

One atomic commit: the lockstep `package.json` files, `pnpm-lock.yaml`, the md-wasm manifest and
synced docs, the island-size contract/README/test pins, the Wind manifest and the five changelog
pages:

```sh
git add packages/*/package.json crates/zfb-md-wasm/npm/package.json pnpm-lock.yaml \
  crates/zfb-md-wasm/shipped-sizes.json crates/zfb-md-wasm/README.md crates/zfb-md-wasm/npm/README.md \
  docs/src/content/docs/api/md-wasm.mdx docs/src/content/docs-ja/api/md-wasm.mdx \
  docs/src/content/docs/guides/browser-markdown-preview.mdx \
  docs/src/content/docs-ja/guides/browser-markdown-preview.mdx \
  docs/src/content/docs/guides/syntax-highlighting.mdx \
  docs/src/content/docs-ja/guides/syntax-highlighting.mdx \
  research/v3-island-size/decision-linux-x64.json research/v3-island-size/README.md \
  scripts/__tests__/island-size-budget.test.mjs docs/public/wind-examples/manifest.json \
  docs/src/content/docs/changelog/zfb/v<version>.mdx \
  docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx \
  docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx \
  docs/src/content/docs/changelog/create-zfb/v<version>.mdx \
  docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx
git status --porcelain   # nothing release-related left unstaged
git commit -m "chore(release): bump to v<version>"
git push origin main
SOURCE_SHA=$(git rev-parse HEAD); SOURCE_TREE=$(git rev-parse HEAD^{tree})
```

### A9. Required CI on the source SHA

```sh
node scripts/release/verify-source-ci.mjs --sha "$SOURCE_SHA" [--json]
```

The source commit must also be an ancestor of `main`: `release.yml`'s release-context refuses a
real tagged run whose source is not reachable from `origin/main` (dry runs skip that check).

It checks the `main` ruleset's required contexts (the list in `scripts/apply-main-ruleset.sh`) on
exactly that SHA. Only GitHub Actions check runs count, and each only when its check suite's
workflow run is the context's own workflow file (for example `health` from
`.github/workflows/health.yml`); a same-named job in another workflow and a commit status with the
same context name are ignored. Exit 0 pass; 1 not passed — pending, cancelled, **skipped**, failed and missing are all
"not passed"; 2 usage or read error. `pnpm audit (prod)` and `Docs gate` run only on pull requests, so on a direct `main` push
they are reported `not-applicable-pr-only`: that is neither a pass nor a failure, and the release
report names it. `--require-pr-only` makes them required (for a source that went through a PR).
Wait and re-run while checks are pending; on a failure, fix on `main`, which produces a
**new** source SHA — repeat [A4](#a4-version-stamped-measurements-in-ci-before-the-bump-reaches-main)
when the fix moves version-stamped inputs, then A8–A9. No draft Release before exit 0.

### A10. Partial state and the draft Release

Re-run [Step 0](#step-0-where-am-i-start-and-after-every-interruption). For the target version:

- **Published Release exists** → stop; the version is live and immutable. A new attempt is a new
  version.
- **Draft exists targeting `$SOURCE_SHA`** → reuse it (its uploaded Darwin inputs stay valid).
- **Draft exists targeting another commit** → it is stale: delete it
  (`gh release delete v<version> --yes`, no `--cleanup-tag`) and recreate. Its Darwin inputs were
  bound to the other source and must be rebuilt.

Also verify the five changelog pages exist at `$SOURCE_SHA` with matching filenames and dates.

Create the draft (one Release per version; notes from five independent extractions):

```sh
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
PRERELEASE_FLAG=$([[ "<version>" =~ -next\.|-beta\.|-rc\. ]] && echo --prerelease || true)
gh release create v<version> --draft --target "$SOURCE_SHA" --title "v<version>" \
  --notes "$RELEASE_NOTES" $PRERELEASE_FLAG
```

Keep these five extractions independent: never reuse one lane's variable as another package's body.
This remains one GitHub Release with the existing tag, binary assets, npm publication order, and
Homebrew flow; only its notes are assembled from the five package sources.

The body never contains the provenance-downgrade marker when an agent creates it. A draft fires no
workflow; GitHub creates the tag ref only when the draft is published.

## Phase M: Darwin inputs on the owner's Mac

Only the owner's Apple Silicon Mac (Rosetta 2 installed) runs this phase. It builds
`aarch64-apple-darwin` natively and `x86_64-apple-darwin` for Rosetta-translated qualification,
from the exact source SHA, and uploads ten assets to the draft: per target the CLI archive and its
`.sha256`, the `zfb-css` addon and its `.sha256`, and `zfb-<version>-<triple>.qualification.json`.

```sh
git fetch origin --tags && git checkout --detach <SOURCE_SHA>
git status --porcelain                         # must be empty
ZFB_X64_NODE=/absolute/path/to/x64/node CARGO_BUILD_JOBS=1 \
  scripts/build-darwin-local.sh --source <SOURCE_SHA> --upload v<version>
```

- `--source` must equal `HEAD`; `--upload` implies `--tag`, which must be `v<packages/zfb version>`.
  Without a tag ref the script binds to the **draft** Release instead: `gh release view` must show
  `isDraft=true` and a `targetCommitish` that is exactly `--source` as a full commit id (a branch
  name or a published Release is refused). That is why A10 creates the draft with
  `--target "$SOURCE_SHA"`. An existing tag ref must still peel to the source. Do not create a tag.
- `ZFB_X64_NODE` is an x64 Node ≥ 24 for the Rosetta addon smoke. `CARGO_BUILD_JOBS=1` suits a
  low-memory Mac. The script honors `HEAVY_GUARD` and the default guard locations, keeps the
  toolchain process-local (`RUSTUP_TOOLCHAIN` or `rust-toolchain.toml`) and refuses a dirty tree,
  an out dir inside the checkout, a non-Apple-Silicon host or missing Rosetta.
- It qualifies each archive's extracted bytes once (no retries), writes both receipts, self-admits
  with `scripts/admit-darwin-artifacts.mjs`, then uploads only to a **draft**, never with
  `--clobber`: identical existing bytes are skipped, different bytes stop the run.
- `--check-only` runs only the argument and source-binding refusals — a fast pre-check.
- A compile timeout means **build incomplete**, not a failed or passed qualification. A receipt with
  `status=failed` is never uploaded.

Record the receipt digests, the `host.arch_label` of each (`native-arm64`,
`rosetta-translated`) and the `sw_vers` line in the release report.

The retired `scripts/build-macos-x64-local.sh` refuses `--upload` (exit 2); it remains only for
`css-carrier-verification.yml` and never produces release inputs.

### Paste-ready Mac prompt

When a non-Mac session finds Darwin inputs missing or rejected (`prepare-darwin`, or a failed
admission in A11), it stops and gives the owner this prompt for a
local session on the Mac (fill in the values):

```text
Phase M of RELEASING.md for zfb v<version>. Source SHA <SOURCE_SHA> (tree <SOURCE_TREE>); the
draft Release v<version> targets it. In a clean checkout of Takazudo/zudo-front-builder: fetch,
check out <SOURCE_SHA> detached (do not create a tag; the script binds to the draft), then run
`ZFB_X64_NODE=<x64 node> CARGO_BUILD_JOBS=1 scripts/build-darwin-local.sh --source <SOURCE_SHA>
--upload v<version>` through the heavy guard if present. Do not publish the Release, do not add the
provenance-downgrade marker unless I decide to, and do not use --clobber. Report the ten uploaded
asset names, both receipts' arch_label and qualification.status, the first result of every check
and the script's final admission line.
```

## Phase A continued: publish and deliver

### A11. Admission and provenance preflight

Download the draft's Darwin assets into an empty temporary directory
(`gh release download v<version> --pattern '*apple-darwin*' --dir <downloaded>`) and run the same
admission as CI. Step 0 runs it against the `draft-target` source too; this direct run is the
cross-check and produces the `admission.json` evidence:

```sh
node scripts/admit-darwin-artifacts.mjs --dir <downloaded> --version <version> \
  --source-commit "$SOURCE_SHA" --source-tree "$SOURCE_TREE" --repo-root . --json-out <evidence-dir>/admission.json
```

Exit 0 admitted, 1 rejected, 2 usage/I/O error. Rejected or missing inputs → [Phase M](#phase-m-darwin-inputs-on-the-owners-mac) (the
owner rebuilds; a conflicting asset must be deleted deliberately by the owner first). Admission
binds bytes to receipts and the source; it cannot prove the receipt's execution — that is the
owner's attested record ([contract](scripts/darwin-artifact-contract.md#what-linux-admission-cannot-prove)).
`release.yml` repeats the admission before any irreversible step and fails naming
`scripts/build-darwin-local.sh` when inputs are missing or invalid.

Then run the provenance preflight for the normal publish mode:

```sh
node scripts/check-provenance-preflight.mjs --version <version> --mode mac-local
```

It checks both `@takazudo/zfb-darwin-arm64` and `@takazudo/zfb-darwin-x64` by default (the optional,
repeatable `--package <name>` narrows it; `release.yml` passes both explicitly).
Exit 0: no downgrade, or every downgrade is covered by the owner's committed policy (it prints an
`ACKNOWLEDGED ... by release/provenance-policy.json` line per package; record them). Exit 1: an
unacknowledged downgrade (see below) — stop unless the owner acknowledges it per run.
Exit 2: registry error, fail closed — retry later; never treat it as acknowledged.

### Provenance contract and the downgrade blocker

npm authentication is **OIDC trusted publishing only**: there is no `NPM_TOKEN` anywhere, and
`scripts/npm-oidc-preflight.mjs` fails the publish job closed without the OIDC environment or with
any stored npm credential. The trusted publisher is `Takazudo` / `zudo-front-builder` /
`release.yml`, no environment, with **Allow npm dist-tag** enabled on each of the eleven packages
([owner runbook](research/npm-oidc-cutover.md)).

Publication is split across two jobs so the npm credential never meets package code:

- **`pack`** (`Pack npm tarballs`, no `id-token`) installs, builds, places the admitted Darwin and
  CI-built binaries in their platform packages, runs the CSS/binary carrier preflight and
  `prepublishOnly`, and packs every package: `npm pack` for the platform packages, `pnpm pack` for
  the workspace packages (so `workspace:` specifiers are rewritten). It uploads the tarballs and a
  manifest with each tarball's sha512.
- **`publish`** (`contents: read` + `id-token: write`) checks out only the control scripts —
  no install, no build, no package code — verifies every tarball's sha512 against the manifest,
  re-inspects every tarball's content itself (`scripts/release/verify-release-tarballs.mjs`: packed
  name/version, 0755 carriers, CSS addons, no `workspace:`, Darwin bytes against the admission
  verdict, MD/WASM against build-wasm-md's manifest) without trusting the pack job, and
  runs `npm publish <tgz>` with an explicit `--provenance` or `--provenance=false` per package, using
  the pinned npm 12.2.1. pnpm never publishes.

The normal publish mode is `mac-local` with
`ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64"`: both Darwin carrier
packages publish with `--provenance=false` (an Actions attestation would falsely claim an Actions
build of bytes built on the Mac) and the nine others with `--provenance`. A package that is already
present keeps its observed provenance as its expectation, so a partial publish followed by a
recovery still verifies. `scripts/verify-npm-provenance-expectations.mjs` then checks each package
version carries exactly its recorded attestation state.

Earlier Darwin versions stay attested forever (`@takazudo/zfb-darwin-arm64` since 1.0.0,
`@takazudo/zfb-darwin-x64` since 2.13.0). npm compares against any earlier attested version, so
**every** local-Darwin release is a permanent trust downgrade for those two packages, not only the
first: consumers with pnpm `trustPolicy: no-downgrade` can never install a new Darwin carrier
version. The preflight in `release.yml` is default-deny and blocks each such release until the
owner acknowledges it by one of two routes:

1. **Per run:** the line `<!-- zfb-release: allow-provenance-downgrade -->` in the draft Release
   body, or the `allow_provenance_downgrade=true` input on a dispatch. A `release.yml` dispatched
   at the tag (the [Actions route](#a12-actions-route-release-coordinator)) reads that line from
   the published Release body. The coordinator checks the same thing before it tags, and never
   sets the input.
2. **Durable policy:** an owner-authored, committed `release/provenance-policy.json`:

   ```json
   {
     "schema": "zfb-provenance-policy/v1",
     "decisions": [
       {
         "package": "@takazudo/zfb-darwin-arm64",
         "unattested_from_version": "6.1.0",
         "reason": "Darwin carriers are built on the owner's Mac (#4687)",
         "decided_by": "<owner>",
         "issue": "https://github.com/Takazudo/zudo-front-builder/issues/<n>"
       }
     ]
   }
   ```

   Every field is required; a malformed file fails the preflight closed. A decision covers only
   `mac-local` mode, only a package that is one of the run's locally built Darwin carriers, and only
   versions `>=` its `unattested_from_version`. CI-built packages, unlisted packages and
   `recovery-no-provenance` are never covered. It is read only from `main`'s committed tree
   (`git show origin/main:release/provenance-policy.json` after fetching `refs/heads/main`), never
   from a working tree, the control checkout or the release source commit. The owner committed it
   on 2026-11-13 for both Darwin carriers from 6.1.1, so a normal `mac-local` release prints two
   `ACKNOWLEDGED` lines, exits 0 and needs no per-run marker; the agent records those lines and
   continues without asking.

**Only the owner does any of these.** No agent, skill or script adds the marker, sets the input, or
creates or edits `release/provenance-policy.json`, even with general release authorization. On a detected downgrade an agent stops,
reports the preflight output (package, version, earlier attested version) and points to
[`research/npm-oidc-cutover.md`](research/npm-oidc-cutover.md#c-darwin-provenance-downgrade-decision).
After the owner acknowledges, re-run the preflight and continue.

### A12. Publish the draft

**Only when Step 0 recommends `publish-draft`** — the single gate. Confirm its source is
`kind: draft-target` at `$SOURCE_SHA`. The direct checks — `verify-source-ci.mjs` exit 0 (A9),
`admit-darwin-artifacts.mjs` exit 0 and a clear or owner-acknowledged preflight (A11) — are a
cross-check: if any disagrees with Step 0, stop and report; they never substitute for
`publish-draft`.

- **Stable:** publish without GitHub Latest in **one** REST update (`make_latest` is a string). A
  bare `gh release edit --draft=false`, or the web UI with "Set as the latest release" checked,
  would make it Latest before delivery is proven:

  ```sh
  RELEASE_ID=$(gh release view v<version> --json databaseId --jq .databaseId)
  printf '%s' '{"draft": false, "prerelease": false, "make_latest": "false"}' \
    | gh api -X PATCH "repos/Takazudo/zudo-front-builder/releases/${RELEASE_ID}" --input -
  gh api repos/Takazudo/zudo-front-builder/releases/latest --jq .tag_name   # still the previous stable
  ```

- **Prerelease:** `gh release edit v<version> --draft=false` (never Latest).

Publishing fires `release: published`, which runs `release.yml` from the tag (control SHA = source
SHA). There is no tag-push flow and never a second Release for the version. Release assets are
never clobbered: a missing asset is uploaded, an identical one skipped, a different one fails the
run naming the asset.

### A12 (Actions route): release coordinator

Use this route when the session cannot push tags or publish the draft itself. Claude Code Cloud's
git proxy refuses tag pushes, and its `gh` may be unauthenticated. It has the same single gate:
Step 0 must recommend `publish-draft`, and A11 must be clear or owner-acknowledged. Never add a
PAT, a second credential or a protection bypass to make another route work.

Dispatch `.github/workflows/release-coordinator.yml` **from `main`**. Use the GitHub MCP server's
workflow dispatch, `gh`, or the Actions UI:

```sh
gh workflow run release-coordinator.yml --ref main \
  -f version=<version> -f sha="$SOURCE_SHA" -f dry_run=true    # read-only gate first
gh workflow run release-coordinator.yml --ref main \
  -f version=<version> -f sha="$SOURCE_SHA" -f dry_run=false
```

`dry_run` defaults to **true** here, the opposite of `release.yml`. Pass it explicitly anyway.

**Inputs:**

- `sha` is the frozen source SHA: the draft's target, normally `main`'s green head when you froze
  it.
  - The coordinator never substitutes another commit. A newer `main` only produces a notice.
  - A resume passes the same SHA again.
  - A different candidate means a new draft, new Phase M inputs and a new version or cleanup.

Its jobs (`scripts/release/coordinate-release.mjs`) re-run the gate before every write, so
re-sending identical inputs resumes and never duplicates:

1. **gate** (read-only). It is `release-status.mjs` plus the candidate contract:
   - **Candidate:** the eleven manifests, the five changelog lanes and the A4 `measuredOnVersion`
     at the SHA. The candidate's own `release.yml` and `node-free-smoke.yml` must carry the
     coordinator contract markers, so an older candidate is refused.
   - **CI:** required CI on the SHA.
   - **Darwin:** admission of the owner-Mac inputs.
   - **Tag and npm:** the tag is absent or already at the SHA, and npm state is consistent.
   - **Provenance:** an unacknowledged downgrade stops it before any write. So do missing Mac
     inputs.
   - **Plan:** it prints `start`, `resume` (the tag or Release already exists at the SHA and no
     run has happened yet) or `reuse` (a run for this tag and SHA is active or succeeded).
2. **tag**: `POST /git/refs` for `refs/tags/v<version>` at the SHA, with the run's `GITHUB_TOKEN`.
   - The same SHA is a no-op. Another SHA is refused. A tag is never moved or deleted.
   - On refusal it records GitHub's response (HTTP status, message, request ID), the candidate,
     control and workflow SHAs, and whether `.github/workflows` differs between the candidate and
     the control commit. It does **not** state a cause.
   - A 403 seen when tagging an older SHA elsewhere (zdtp) has an unconfirmed explanation. Report
     the recorded facts. The owner decides whether to push the tag from a user credential or pick
     another candidate.
3. **publish**: publishes the draft in one PATCH with `make_latest: "false"`. It never edits the
   body. A Release published by `GITHUB_TOKEN` starts no workflow.
4. **dispatch**: runs `release.yml` **at ref `v<version>`** with `dry_run=false` and no other
   input.
   - At the tag, control SHA = source SHA and the mode is `mac-local`, exactly like
     `release: published`. The trusted publisher (`release.yml`, no environment) and provenance
     stay honest.
   - An active or successful run for the tag and SHA is reused.
   - A failed one is **not** re-run: follow Step 0 (`dispatch-recovery`/`dispatch-finalize`, from
     `main`, as before). The same applies once any recovery or finalize run exists.
   - Run history comes from the coordinator's own listing: every real run on the tag ref, plus
     dispatches from `main` that carry the tag's run name. Dry runs are filtered out before any
     limit applies, and a listing that reaches its limit is inconclusive.
   - Immediately before the dispatch it lists the runs again and refuses if one has appeared.
   - The gate logs the SHA-256 of the Release body it checked, and the tag-dispatched
     `release.yml` logs the digest of the body it read. Compare the two when auditing an
     acknowledgement.

Then continue at A13. The run is named `Release v<version>`, with event `workflow_dispatch` and
head branch `v<version>`.

### A13. Watch the runs

1. Find the `release.yml` run (head branch `v<version>`) and record its run ID and attempt. Its
   event is `release`, or `workflow_dispatch` on the Actions route:

   ```sh
   gh run list --workflow release.yml --limit 5 \
     --json databaseId,attempt,headBranch,headSha,status,conclusion \
     --jq '.[] | select(.headBranch == "v<version>")'
   ```

2. Poll it to completion in the background (`gh run view <id> --json status,conclusion`). Its run
   name is exactly `Release v<version>`. Jobs: `release-context`, `source-ci`,
   `provenance-preflight`, `admit-darwin`, `build` (Linux, Windows), `build-wasm-md`,
   `release-assets`, `pack` (`Pack npm tarballs`), `publish`, `md-wasm-asset`
   (`Upload MD/WASM digest asset`), `smoke-clean-room`, `notify`.
3. **Stable:** a successful Release run triggers `node-free-smoke.yml` by `workflow_run`. Record
   the run whose `Smoke released (install.sh)` job installed the exact tag. After a rerun, use the
   run the final attempt triggered.

   **Actions route:** it is not established whether `workflow_run` follows a `release.yml` run that
   `GITHUB_TOKEN` dispatched. If no `workflow_run` installer for the tag appears a few minutes
   after the Release run succeeds, dispatch it at the tag, once:

   ```sh
   gh workflow run node-free-smoke.yml --ref refs/tags/v<version> -f release_tag=v<version>
   ```

   Only `smoke-released` runs, from the tag's commit. Record which kind it was.
4. Do not rely on follow-up workflows being triggered by events that `GITHUB_TOKEN` created; any
   follow-up this procedure needs is an explicit `gh workflow run` dispatch (below).
5. On failure: `gh run view <id> --log-failed`. A clearly transient failure (runner eviction,
   network) may be rerun once (`gh run rerun <id> --failed`) — record the first result and the
   retry. Otherwise go to Step 0 and follow its recommendation. Never unpublish or delete the
   Release.
6. Every package is published under the transient `release-pending` dist-tag, never under
   `latest` or `next`. The `publish` job moves the channel tag (`latest` for a stable,
   `next` for a prerelease, plus `latest` with `ALSO_LATEST`) only after its provenance
   verification passes for all eleven packages, through `scripts/promote-release-channel.sh`.
   If npm keeps a package in pre-publication processing, the verifier exits 3, the run fails and
   no channel tag moves on any package. Go to
   [npm is still processing a package](#npm-is-still-processing-a-package).

### A14. Verify delivery

For a stable release, from this session (never as a `release.yml` step):

```sh
node scripts/verify-release-delivery.mjs --version <version> \
  --release-run-id <release-run-id> --installer-run-id <installer-run-id> \
  --expected-source "$SOURCE_SHA" --json <evidence-dir>/release-delivery-<version>.json
```

GET-only (prefix `NODE_USE_ENV_PROXY=1` behind an `HTTPS_PROXY`). It requires the release run's
`Pack npm tarballs` and `Upload MD/WASM digest asset` jobs to have succeeded, alongside the build,
asset and publish jobs. Exit 0 verified; 1 definite
failure; 2 incomplete evidence (never a pass; re-run once if transient); 3 usage error. Also
recompute the four `.wasm` SHA-256 digests from the published `@takazudo/zfb-md-wasm@<version>`
tarball and compare with its `dist/shipped-artifacts.json` and the Release asset
`zfb-md-wasm-<version>-shipped-artifacts.json`.

The verifier accepts the normal `mac-local` run, started either by `release: published` or by
the coordinator's dispatch at the tag (`tag-dispatch`). Each run's provenance must name its own
trigger (`release` or `workflow_dispatch`). The installer run must be a `workflow_run` or a
dispatch at exactly the tag commit. Both Darwin carriers must carry **no**
attestation and their packed `zfb` and `zfb-css.node` must equal the Release's Darwin archive
binary and addon bytes, while every other package must be attested. Recovery, finalize and dry-run
runs, and runs from before #4687 (`legacy-hosted-darwin`, `legacy-fast-mac`), are classified,
never accepted — they cannot reach exit 0.

Per-package npm identity can be checked independently:

```sh
node scripts/release/npm-package-state.mjs --version <version> --source-commit "$SOURCE_SHA" \
  --admission <evidence-dir>/admission.json [--json]
```

Each package is `absent` (definitive E404/ETARGET on bounded re-reads), `present-verified`
(attestation `gitCommit`, else `gitHead`, else for a Darwin carrier the binary/addon digest from the
admission verdict, equals the source), `present-unverified` (fail closed) or `uncertain`
(network/5xx/auth; original error kept). Only `present-verified` counts.

On exit 1 or a persistent 2: leave the Release published but not Latest, skip Homebrew, report.
npm versions are immutable — never unpublish.

### A15. Promote, Homebrew, read back (stable only, after exit 0)

```sh
printf '%s' '{"make_latest": "true"}' \
  | gh api -X PATCH "repos/Takazudo/zudo-front-builder/releases/${RELEASE_ID}" --input -
gh api repos/Takazudo/zudo-front-builder/releases/latest --jq .tag_name   # v<version>
./scripts/update-homebrew-formula.sh v<version> --push
```

The Homebrew script needs a checkout of `Takazudo/homebrew-tap` (`--tap-path`, `ZFB_TAP_PATH`, or a
clone from `ZFB_TAP_REMOTE`) and push access to it. It is idempotent; retry once at most. A session
without tap credentials (typical in Cloud) reports the exact command for the owner instead. A
Homebrew failure does not undo a verified release; report it separately.

Then read the channels back and report them **separately** from delivery:

```sh
node scripts/verify-release-delivery.mjs --version <version> \
  --release-run-id <release-run-id> --installer-run-id <installer-run-id> \
  --expected-source "$SOURCE_SHA" --expect-github-latest --expect-npm-latest --expect-homebrew \
  --json <evidence-dir>/release-delivery-<version>-channels.json
```

Prereleases skip the installer smoke, the delivery verifier, Latest and Homebrew; report
`npm view @takazudo/zfb dist-tags`. Testers use `npm i -g @takazudo/zfb@next` or `ZFB_VERSION=latest-prerelease`.

## Workflow dispatches: recovery, finalize, dry run

`release.yml`'s `workflow_dispatch` inputs are `dry_run`, `release_tag`, `allow_provenance_downgrade`
and `finalize_only`. For recovery, finalize and dry runs, always dispatch from `--ref main` (a `v*`
ref would load that tag's old workflow). The only tag-ref dispatch is the coordinator's first run
of a new candidate, whose own workflow is the right one; always pass `dry_run` explicitly (**its default is false**), and record the control SHA
(`main`'s head) next to the source SHA.

### Recovery: a published Release whose build or publish failed

When Step 0 recommends `dispatch-recovery` (Release published, packages or build assets missing).
If the cause is in `release.yml`, fix it on `main` first through the normal review and CI.
Recovery checks out the tag's verified commit but runs `main`'s workflow. The publish mode depends
on the control commit:

- **Control commit = source commit** (`main`'s head is still the release commit): GitHub OIDC
  identifies the source truthfully, so recovery uses the normal `mac-local` mode with honest
  provenance and the normal acknowledgement routes for the Darwin downgrade.
- **Control commit ≠ source commit**: OIDC would identify the wrong commit, so it uses
  `recovery-no-provenance` (packages not yet present publish unattested). That is a trust downgrade
  for every package an earlier version attested; the committed policy never covers it, so it always
  needs `allow_provenance_downgrade=true` — an **owner-only** input.

Packages already present keep their observed provenance, so a mixed partial-then-recovery release
still verifies its expectations. An agent prints the command and stops; the owner runs it:

```sh
gh workflow run release.yml --ref main \
  -f dry_run=false -f release_tag=v<version> -f allow_provenance_downgrade=true
```

Watch it like [A13](#a13-watch-the-runs). A recovered stable is not accepted by the delivery
verifier as a normal run (its name is `Release v<version>` from a `workflow_dispatch`), so its
Latest promotion is an owner decision. Afterwards ship the next
patch through the normal path to restore provenance (#2623), and point affected consumers at the
`ERR_PNPM_TRUST_DOWNGRADE` troubleshooting entry.

### Finalize only: dist-tags, digest asset, verification

When Step 0 recommends `dispatch-finalize` (all eleven packages `present-verified` and all Release
assets present, but a post-publish step did not complete):

```sh
gh workflow run release.yml --ref main -f dry_run=false -f release_tag=v<version> -f finalize_only=true
```

`finalize_only` needs `release_tag` and a dispatch from `main`. With `dry_run=true` it runs only the
read-only gate (all eleven packages `present-verified`, the six hosted assets and ten Darwin inputs
on the Release, provenance expectations) and skips the dist-tag changes, digest upload and smoke —
useful as a check, never as finalization. With `dry_run=false`, no build, no publish: the
`finalize` job (`id-token: write`) reconciles dist-tags (advance `latest` when
`scripts/release/resolve-also-latest.mjs` says so; retire `next` where it is no longer ahead) and
verifies the per-package provenance expectations (observed state of each present package); the
separate `finalize-assets` job (`contents: write`, no `id-token`) uploads the md-wasm digest asset if
missing; then the smoke and notify run. The run is named `Release v<version> (finalize only)`. It is
idempotent. If build assets are
missing it stops `blocked`; use recovery instead.

Finalize is also where a release that stopped on npm processing completes. The `finalize` job runs
`scripts/promote-release-channel.sh` (the same promotion as the `publish` job): it refuses unless
all eleven packages are `present-verified` for the source commit, then moves the channel tag on
all packages together. The sweep (`retire-next-dist-tag.mjs`) then removes `next` where it is no
longer ahead and `release-pending` where the channel tag has reached it. A newer `release-pending`
is a later release in flight and is kept.

Finalizing a release that a newer version has already superseded fails with
`refusing to move 'latest' backwards`. That is deliberate fail-closed behaviour; the sweep still
clears `release-pending`. Do not work around it.

### npm is still processing a package

npm sometimes accepts a publish and holds the version in pre-publication processing, so it keeps
answering E404 (#4711 is the precedent: large carriers took 6 to 12 minutes, and a stuck one
needs npm support).

**Signs**

- The `Verify npm provenance expectations` step exits 3 and the job summary shows
  "npm accepted the publish but is still processing", listing the packages.
- Step 0 recommends `wait-npm-processing`. The package state is `processing`, never `absent`.

**What to do**

1. Stop and report. Do not recover.
2. Check the status API locally with your own npm login. `{"status":"validating"}` means still
   processing. Never print the credential:

   ```sh
   TOKEN="$(sed -n 's|^//registry.npmjs.org/:_authToken=||p' "$HOME/.npmrc")"
   curl -s -H "Authorization: Bearer ${TOKEN}" \
     "https://registry.npmjs.org/-/package/<name, with / as %2f>/version/<version>/status"
   ```

3. Wait up to 24 hours. If a package is still processing after that, contact npm support.
4. Once every package is visible, finalize from `main`:

   ```sh
   gh workflow run release.yml --ref main -f release_tag=vX.Y.Z -f finalize_only=true
   ```

   Re-run Step 0 first: it must recommend `dispatch-finalize`.

**Never**

- Never republish (npm answers E409 "previously staged"), and never cut a new version to work
  around the wait.
- Never move `latest` or `next` by hand. Only `promote-release-channel.sh` moves them, after
  verification.

### Dry run

`gh workflow run release.yml --ref <base/... or feature/... branch> -f dry_run=true` exercises the
build, packaging and checksum steps without uploading Release assets, changing dist-tags or
publishing to npm. With a `release_tag` the `pack` job also packs the npm tarballs (an Actions
artifact upload). The `publish`, `md-wasm-asset` and `finalize-assets` jobs are skipped. Dry runs
queue in their own concurrency group (`release-dry-run-<tag>`), so they never wait behind or block a
real run; the run name ends in ` (dry run)`. It still uploads Actions artifacts and may send the CI
failure alert. It validates the workflow at that
source only — never the version-stamped measurements or a later version's gates. Skipped steps in a
dry run are not passes, and a dry run never proves OIDC publication or dist-tag writes.

## Cancel and clean up

For an abandoned release (draft created, never published) or an orphan found in A1:

1. Confirm it is a draft (`gh release view v<version> --json isDraft --jq .isDraft` → `true`), then
   `gh release delete v<version> --yes` — this also removes its uploaded Darwin inputs. No
   `--cleanup-tag` (a draft has no tag ref; the flag fails with HTTP 422 after the deletion). Never
   delete a **published** Release.
2. Bump commit: if it is still `main`'s head (`git rev-list --count <SOURCE_SHA>..origin/main` is
   `0`), `git revert --no-edit <SOURCE_SHA>` and push (one revert removes the version, lockfile and
   five pages). If later commits sit on top, leave it; the next release supersedes the version.

## Failure recovery quick reference

| Situation | Action |
| --- | --- |
| Lockfile drift beyond version lines | stop, surface the diff |
| A7 focused check fails | stop before committing; fix and restart |
| Required CI not passed on the source SHA | fix on `main` → new source SHA → redo from A4/A8 |
| Darwin inputs missing/rejected | Phase M (Cloud: hand off and stop) |
| Preflight downgrade, no acknowledgement | stop; owner decision |
| Preflight registry error | retry later; fail closed |
| Draft targets a stale commit | delete and recreate; rebuild Darwin inputs |
| `release.yml` failed after publish | Step 0 → `dispatch-recovery` (owner) or `dispatch-finalize` |
| Asset conflict (different bytes) | stop; never clobber; owner decides |
| Delivery verifier exit 1 / persistent 2 | Release stays not-Latest; no Homebrew; report |
| Homebrew push fails | release still delivered; report the command for a retry |
| Coordinator stopped in **gate** | nothing was written; fix the named cause, re-send the same inputs |
| Coordinator `OWNER_ACK_REQUIRED` | stop; owner decision (A11) |
| Coordinator `TAG_CREATE_FAILED` | report its recorded diagnostics; no PAT, no bypass; owner decides |
| Coordinator stopped after **tag** or **publish** | re-send the same inputs (it resumes) |
| Coordinator `RUN_FAILED` | Step 0 → recovery/finalize from `main` |
| Coordinator `RECOVERY_STARTED` | a recovery/finalize run already exists; Step 0 decides, not the coordinator |
| Coordinator `RUN_APPEARED` or `DISPATCH_READBACK_PENDING` | `gh run list --workflow release.yml --branch v<version>` first; a re-send reuses a listed run, never dispatches a second |
| Coordinator `RUNS_TRUNCATED` (exit 3) | the run history could not be read completely; do not treat it as "never ran" |
| No installer run after a coordinated release | dispatch `node-free-smoke.yml` at the tag (A13) |

## Release report

Report with verdict headings: the version and channel; source SHA and tree; tested checkout;
control SHA for any dispatch; Darwin receipt digests and arch labels (x86_64 **Rosetta-translated**);
admission verdict; preflight result and who acknowledged any downgrade; every run/attempt/job ID
with its first result and retries; delivery verifier exit and JSON path; per-package npm state;
GitHub Latest, dist-tags and Homebrew read-back reported separately; anything skipped or blocked.
