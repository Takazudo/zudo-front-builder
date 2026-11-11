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
`.permissions`) and whether Actions workflows are readable. It never dispatches anything. Phase A
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
  `release: published` event it is the source SHA. For a `workflow_dispatch` recovery or finalize
  run from `main` it is `main`'s head at dispatch, which differs from the source SHA; record both.
- **Run evidence** — every run ID, attempt number, job ID and conclusion, first results and all
  retries; the Darwin receipts' digests and the admission verdict.

## Step 0: where am I? (start and after every interruption)

```sh
node scripts/release/release-status.mjs --tag v<version> [--json]
```

Read-only. It reports the tag → commit/tree, the Release (absent/draft/published,
`targetCommitish`), the expected Release assets (including the ten Darwin inputs) as present,
missing or conflicting, the source commit's required CI state, a Darwin admission of the
downloaded assets, per-package npm state, dist-tags, the latest `release.yml` runs for the tag
(run/attempt IDs and conclusions), and **one** recommended next action:

| Recommendation | Go to |
| --- | --- |
| `prepare-darwin` | [Phase M](#phase-m-darwin-inputs-on-the-owners-mac) (Cloud: hand off and stop) |
| `publish-draft` | [A11](#a11-admission-and-provenance-preflight) then [A12](#a12-publish-the-draft) |
| `dispatch-recovery` | [Recovery](#recovery-a-published-release-whose-build-or-publish-failed) |
| `dispatch-finalize` | [Finalize](#finalize-only-dist-tags-digest-asset-verification) |
| `blocked:<reason>` | stop and report the reason |
| `complete` | [A14](#a14-verify-delivery) onward if not yet done, else report |

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
- Assign every package-facing change to its lane(s) per the
  [five-lane contract](CLAUDE.md#five-lane-release-changelog-contract): `zfb` (engine/CLI,
  `@takazudo/zfb`, `@takazudo/zfb-slugify`, native carriers), `zfb-runtime`,
  `zfb-adapter-cloudflare`, `create-zfb`, `zfb-md-wasm`. Duplicate cross-package changes into
  every affected lane; omit repo-only docs/tests/CI/maintenance.

Print the proposal (current → new, categorized commits) for visibility.

### A4. Version-stamped measurements in CI (before the bump reaches `main`)

Three gates move with the version string, so every release version is measured in CI first:

- **md-wasm** — the four `.wasm` files embed `ZFB_RELEASE_VERSION`; exact sizes can move with the
  stamp. Never measure or refresh `shipped-sizes.json` from a Mac build
  ([`LOCAL-VS-CI-SIZES.md`](crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md)).
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
     (keep ceilings); run `node scripts/assert-md-wasm-size-docs.mjs --fix`, `pnpm format:mdx`,
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

Create exactly these English pages (no Japanese mirrors):

```text
docs/src/content/docs/changelog/zfb/v<version>.mdx
docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx
docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx
docs/src/content/docs/changelog/create-zfb/v<version>.mdx
docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx
```

```mdx
---
title: 'v<version>'
sidebar_position: <computed>
# only on a lane's first-ever page:
pagination_next: null
---

# v<version>

Released: <YYYY-MM-DD>

## Breaking Changes

- description (hash)
```

- Sections only when they have entries; an unchanged lane's body after `Released:` is exactly a
  blank line and `- No package-specific changes.`; never borrow another lane's narrative.
- `sidebar_position` per lane, computed as if the page were absent — one call per target:
  `node scripts/next-changelog-sidebar-position.mjs docs/src/content/docs/changelog/<lane>/v<version>.mdx`.
  Never use the retired root changelog path, a global maximum or the old mega-number formula. An
  empty lane starts at 1 with `pagination_next: null`.
- A `zfb-md-wasm` note never says artifacts are "unchanged" without saying what that covers: every
  release moves all four `.wasm` SHA-256 digests (#2885). Point to the Release asset
  `zfb-md-wasm-<version>-shipped-artifacts.json` and the export
  `@takazudo/zfb-md-wasm/shipped-artifacts.json`; no digest tables.

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
  docs/src/content/docs/changelog/*/v<version>.mdx
git status --porcelain   # nothing release-related left unstaged
git commit -m "chore(release): bump to v<version>"
git push origin main
SOURCE_SHA=$(git rev-parse HEAD); SOURCE_TREE=$(git rev-parse HEAD^{tree})
```

### A9. Required CI on the source SHA

```sh
node scripts/release/verify-source-ci.mjs --sha "$SOURCE_SHA" [--json]
```

It checks the `main` ruleset's required contexts (the list in `scripts/apply-main-ruleset.sh`) on
exactly that SHA. Exit 0 pass, 1 not passed (pending, cancelled or failed are all "not passed"),
2 error. Wait and re-run while checks are pending; on a failure, fix on `main`, which produces a
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
for lane in zfb zfb-runtime zfb-adapter-cloudflare create-zfb zfb-md-wasm; do
  sed -n '/^Released:/,$ p' "docs/src/content/docs/changelog/$lane/v<version>.mdx" > "$TMPDIR/notes-$lane.md"
done
{
  printf '## @takazudo/zfb\n\n';                   cat "$TMPDIR/notes-zfb.md";                    printf '\n'
  printf '## @takazudo/zfb-runtime\n\n';           cat "$TMPDIR/notes-zfb-runtime.md";            printf '\n'
  printf '## @takazudo/zfb-adapter-cloudflare\n\n'; cat "$TMPDIR/notes-zfb-adapter-cloudflare.md"; printf '\n'
  printf '## create-zfb\n\n';                      cat "$TMPDIR/notes-create-zfb.md";             printf '\n'
  printf '## @takazudo/zfb-md-wasm\n\n';           cat "$TMPDIR/notes-zfb-md-wasm.md"
} > "$TMPDIR/release-notes.md"
PRERELEASE_FLAG=$([[ "<version>" =~ -next\.|-beta\.|-rc\. ]] && echo --prerelease || true)
gh release create v<version> --draft --target "$SOURCE_SHA" --title "v<version>" \
  --notes-file "$TMPDIR/release-notes.md" $PRERELEASE_FLAG
```

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
git tag v<version> <SOURCE_SHA>                # LOCAL tag only — do not push; the draft has no tag ref yet
ZFB_X64_NODE=/absolute/path/to/x64/node CARGO_BUILD_JOBS=1 \
  scripts/build-darwin-local.sh --source <SOURCE_SHA> --upload v<version>
```

- `--source` must equal `HEAD`; `--upload` implies `--tag`, which must be `v<packages/zfb version>`
  and peel locally to the source. The local tag satisfies that binding; publishing the draft later
  creates the remote tag at the same commit.
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
`rosetta-translated`) and the `sw_vers` line in the release report. Afterwards delete the local
tag if desired (`git tag -d v<version>`); it is not needed again.

### Paste-ready Mac prompt

When a non-Mac session reaches `prepare-darwin`, it stops and gives the owner this prompt for a
local session on the Mac (fill in the values):

```text
Phase M of RELEASING.md for zfb v<version>. Source SHA <SOURCE_SHA> (tree <SOURCE_TREE>); the
draft Release v<version> targets it. In a clean checkout of Takazudo/zudo-front-builder: fetch,
check out <SOURCE_SHA> detached, create the LOCAL tag v<version> at it (do not push), then run
`ZFB_X64_NODE=<x64 node> CARGO_BUILD_JOBS=1 scripts/build-darwin-local.sh --source <SOURCE_SHA>
--upload v<version>` through the heavy guard if present. Do not publish the Release, do not add the
provenance-downgrade marker unless I decide to, and do not use --clobber. Report the ten uploaded
asset names, both receipts' arch_label and qualification.status, the first result of every check
and `node scripts/release/release-status.mjs --tag v<version>`'s recommendation.
```

## Phase A continued: publish and deliver

### A11. Admission and provenance preflight

Re-run Step 0. It downloads the draft's Darwin assets into a temporary directory and runs the same
admission as CI:

```sh
node scripts/admit-darwin-artifacts.mjs --dir <downloaded> --version <version> \
  --source-commit "$SOURCE_SHA" --source-tree "$SOURCE_TREE" --repo-root . --json-out <evidence-dir>/admission.json
```

Exit 0 admitted, 1 rejected, 2 usage/I/O error. Rejected or missing inputs → `prepare-darwin` (the
owner rebuilds; a conflicting asset must be deleted deliberately by the owner first). Admission
binds bytes to receipts and the source; it cannot prove the receipt's execution — that is the
owner's attested record ([contract](scripts/darwin-artifact-contract.md#what-linux-admission-cannot-prove)).
`release.yml` repeats the admission before any irreversible step and fails naming
`scripts/build-darwin-local.sh` when inputs are missing or invalid.

Then run the provenance preflight for the normal publish mode:

```sh
node scripts/check-provenance-preflight.mjs --version <version> --mode mac-local
```

Exit 0: no downgrade. Exit 1: a downgrade (see below) — stop unless the owner has acknowledged it.
Exit 2: registry error, fail closed — retry later; never treat it as acknowledged.

### Provenance contract and the downgrade blocker

npm authentication is **OIDC trusted publishing only**: there is no `NPM_TOKEN` anywhere, and
`scripts/npm-oidc-preflight.mjs` fails the publish job closed without the OIDC environment or with
any stored npm credential. The trusted publisher is `Takazudo` / `zudo-front-builder` /
`release.yml`, no environment, with **Allow npm dist-tag** enabled on each of the eleven packages
([owner runbook](research/npm-oidc-cutover.md)).

The normal publish mode is `mac-local` with
`ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64"`: both Darwin carrier
packages publish with `--provenance=false` (an Actions attestation would falsely claim an Actions
build of bytes built on the Mac) and the nine others with `--provenance`. After publishing,
`scripts/verify-npm-provenance-expectations.mjs` checks every version carries exactly the intended
attestation state.

Earlier versions (through 6.0.1) published attested Darwin packages, so the **first** local-Darwin
release is a permanent npm trust downgrade for those two packages: consumers with pnpm
`trustPolicy: no-downgrade` cannot install that version. The preflight in `release.yml` is
default-deny and blocks it until the owner acknowledges, by one of:

- adding the line `<!-- zfb-release: allow-provenance-downgrade -->` to the draft Release body, or
- the `allow_provenance_downgrade=true` input on a dispatch.

**Only the owner does either.** No agent, skill or script adds the marker or sets the input on the
owner's behalf, even with general release authorization. On a detected downgrade an agent stops,
reports the preflight output (package, version, earlier attested version) and points to
[`research/npm-oidc-cutover.md`](research/npm-oidc-cutover.md#c-darwin-provenance-downgrade-decision).
After the owner acknowledges, re-run Step 0 and continue.

### A12. Publish the draft

Only when Step 0 recommends `publish-draft` (CI passed on the source SHA, all Darwin inputs
admitted, no unacknowledged downgrade).

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

### A13. Watch the runs

1. Find the `release.yml` run (event `release`, head branch `v<version>`) and record its run ID and
   attempt:

   ```sh
   gh run list --workflow release.yml --event release --limit 5 \
     --json databaseId,attempt,headBranch,headSha,status,conclusion \
     --jq '.[] | select(.headBranch == "v<version>")'
   ```

2. Poll it to completion in the background (`gh run view <id> --json status,conclusion`). It admits
   the Darwin inputs, builds Linux and Windows, uploads assets and publishes the eleven packages.
3. **Stable:** a successful Release run triggers `node-free-smoke.yml` by `workflow_run`; record the
   run whose `Smoke released (install.sh)` job installed the exact tag (after a rerun, the run the
   final attempt triggered).
4. Do not rely on follow-up workflows being triggered by events that `GITHUB_TOKEN` created; any
   follow-up this procedure needs is an explicit `gh workflow run` dispatch (below).
5. On failure: `gh run view <id> --log-failed`. A clearly transient failure (runner eviction,
   network) may be rerun once (`gh run rerun <id> --failed`) — record the first result and the
   retry. Otherwise go to Step 0 and follow its recommendation. Never unpublish or delete the
   Release.

### A14. Verify delivery

For a stable release, from this session (never as a `release.yml` step):

```sh
node scripts/verify-release-delivery.mjs --version <version> \
  --release-run-id <release-run-id> --installer-run-id <installer-run-id> \
  --expected-source "$SOURCE_SHA" --json <evidence-dir>/release-delivery-<version>.json
```

GET-only (prefix `NODE_USE_ENV_PROXY=1` behind an `HTTPS_PROXY`). Exit 0 verified; 1 definite
failure; 2 incomplete evidence (never a pass; re-run once if transient); 3 usage error. Also
recompute the four `.wasm` SHA-256 digests from the published `@takazudo/zfb-md-wasm@<version>`
tarball and compare with its `dist/shipped-artifacts.json` and the Release asset
`zfb-md-wasm-<version>-shipped-artifacts.json`.

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
and `finalize_only`. Always dispatch from `--ref main` (a `v*` ref would load that tag's old
workflow), always pass `dry_run` explicitly (**its default is false**), and record the control SHA
(`main`'s head) next to the source SHA.

### Recovery: a published Release whose build or publish failed

When Step 0 recommends `dispatch-recovery` (Release published, packages or build assets missing).
If the cause is in `release.yml`, fix it on `main` first through the normal review and CI.
Recovery checks out the tag's verified commit but runs `main`'s workflow, so GitHub OIDC identifies
the `main` commit: it publishes in `recovery-no-provenance` mode (no package attested), which is a
trust downgrade for every package an earlier version attested. It therefore requires
`allow_provenance_downgrade=true` — an **owner-only** input. An agent prints the command and stops;
the owner runs it:

```sh
gh workflow run release.yml --ref main \
  -f dry_run=false -f release_tag=v<version> -f allow_provenance_downgrade=true
```

Watch it like [A13](#a13-watch-the-runs). A recovered stable is not accepted by the delivery
verifier as a normal run, so its Latest promotion is an owner decision. Afterwards ship the next
patch through the normal path to restore provenance (#2623), and point affected consumers at the
`ERR_PNPM_TRUST_DOWNGRADE` troubleshooting entry.

### Finalize only: dist-tags, digest asset, verification

When Step 0 recommends `dispatch-finalize` (all eleven packages `present-verified` and all Release
assets present, but a post-publish step did not complete):

```sh
gh workflow run release.yml --ref main -f dry_run=false -f release_tag=v<version> -f finalize_only=true
```

No build, no publish: it reconciles dist-tags (advance `latest` when the `ALSO_LATEST` condition
applies; retire `next` where it is no longer ahead), uploads the md-wasm digest asset if missing,
verifies provenance expectations, runs the smoke and notifies. It is idempotent. If build assets are
missing it stops `blocked`; use recovery instead.

### Dry run

`gh workflow run release.yml --ref <base/... or feature/... branch> -f dry_run=true` exercises the
build, packaging and checksum steps without uploading Release assets or publishing to npm. It still
uploads Actions artifacts and may send the CI failure alert. It validates the workflow at that
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
3. On the Mac, delete the local tag (`git tag -d v<version>`) if one was created.

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

## Release report

Report with verdict headings: the version and channel; source SHA and tree; tested checkout;
control SHA for any dispatch; Darwin receipt digests and arch labels (x86_64 **Rosetta-translated**);
admission verdict; preflight result and who acknowledged any downgrade; every run/attempt/job ID
with its first result and retries; delivery verifier exit and JSON path; per-package npm state;
GitHub Latest, dist-tags and Homebrew read-back reported separately; anything skipped or blocked.
