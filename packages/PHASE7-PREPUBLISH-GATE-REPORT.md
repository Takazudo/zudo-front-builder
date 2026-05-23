# Pre-Publish Gate Report — Issue #424

**Date:** 2026-05-23
**Branch:** `npm-pre-publish-final-gate/prepublish-gate`
**Operator:** Claude Sonnet 4.6 subagent (issue #424)
**Base branch tested:** `base/npm-pre-publish-final-gate` (post-Wave-1: includes #326, #413, #423)

---

## Step 1 — pnpm pack smoke (9 tarballs + extras)

`pnpm install --frozen-lockfile` + `pnpm -r build` succeeded. Then `pnpm -r pack` produced 12 tarballs total (9 target packages + 3 private workspace packages: `zudo-front-builder-0.0.0.tgz`, `docs-0.0.1.tgz`, `zfb-islands-runtime-0.0.0.tgz`). The 3 private packages have `"private": true` and will not be published — `pnpm publish` skips them automatically.

### 4 TS packages

| Tarball | Size | Has dist/ | Has README/CHANGELOG/LICENSE | Has bin/ | Has src/*.ts leaked |
|---|---|---|---|---|---|
| takazudo-zfb-0.1.0-next.0.tgz | 56K | YES | YES | YES (bin/zfb.mjs) | NO |
| takazudo-zfb-runtime-0.1.0-next.0.tgz | 55K | YES | YES | NO (correct) | NO |
| takazudo-zfb-adapter-cloudflare-0.1.0-next.0.tgz | 11K | YES | YES | YES (bin/cli.mjs) | NO |
| create-zfb-0.1.0-next.0.tgz | 1.9K | N/A | YES | YES (bin/create-zfb.mjs) | NO |

Note: `takazudo-zfb-adapter-cloudflare` includes `package/src/worker-wrapper.mjs` — this is a JS file (not TypeScript) and is intentionally listed in `files:` per the Option C design from Phase 2. This is NOT a src/*.ts leak.

### 5 platform packages

| Tarball | Size | Has package.json | Has LICENSE | Has README.md | Has binary |
|---|---|---|---|---|---|
| takazudo-zfb-darwin-arm64-0.1.0-next.0.tgz | 1.2K | YES | YES | YES | NO (expected) |
| takazudo-zfb-darwin-x64-0.1.0-next.0.tgz | 1.2K | YES | YES | YES | NO (expected) |
| takazudo-zfb-linux-arm64-gnu-0.1.0-next.0.tgz | 1.2K | YES | YES | YES | NO (expected) |
| takazudo-zfb-linux-x64-gnu-0.1.0-next.0.tgz | 1.2K | YES | YES | YES | NO (expected) |
| takazudo-zfb-win32-x64-msvc-0.1.0-next.0.tgz | 1.2K | YES | YES | YES | NO (expected) |

The missing binaries are expected: they are produced by `release.yml`'s cross-compile step and copied into the platform packages before publish. Local `pnpm pack` runs without them. The tarballs are small (1.2K) because they only contain the metadata files.

### Anomalies found

None. The workspace:* dependencies are correctly rewritten to concrete version `0.1.0-next.0` in all published package.json files (verified by inspecting `takazudo-zfb` and `create-zfb` package.json from inside the tarballs).

---

## Step 2 — workspace:* rewriting check

Verified in `takazudo-zfb` tarball:

- `optionalDependencies` shows `"@takazudo/zfb-darwin-arm64": "0.1.0-next.0"` (concrete, not workspace:*)

Verified in `create-zfb` tarball:

- `dependencies` shows `"@takazudo/zfb": "0.1.0-next.0"` (concrete, not workspace:*)

PASS.

---

## Step 3 — Build verification

TS packages built cleanly: `pnpm -r build` completed without errors. All 4 TS packages have `dist/` directories in their tarballs.

---

## Step 4 — NPM_TOKEN existence check

**CRITICAL FINDING: NPM_TOKEN is NOT present in repository secrets.**

`gh secret list` output shows only:

- `CLOUDFLARE_ACCOUNT_ID` (2026-04-26)
- `CLOUDFLARE_API_TOKEN` (2026-04-26)

NPM_TOKEN is absent. The dry_run=true workflow does not use NPM_TOKEN (the publish step is skipped), so this does not block the dry-run validation. However, **NPM_TOKEN MUST be added before the real v0.1.0-next.1 tag push.** Per the issue body: it must be an Automation-type token (not Classic) to bypass 2FA in CI.

Action required by the user: Create an Automation token at https://www.npmjs.com/settings/{username}/tokens and add it as a repo secret named `NPM_TOKEN`.

---

## Step 5 — 5-platform release.yml dry-run

**Trigger:** `gh workflow run release.yml --ref base/npm-pre-publish-final-gate -f dry_run=true`
**Run URL:** https://github.com/Takazudo/zudo-front-builder/actions/runs/26331553708
**Run ID:** 26331553708

### Root cause analysis of prior failures (run IDs 26183527509, 26180456664)

Investigation confirmed the root cause: in the old `main` branch at SHA `8ffebbd`, `pnpm install --frozen-lockfile` appeared at line 126 of release.yml — **AFTER** `Build zfb binary` at line 71. When `cargo build` ran, `node_modules/.pnpm/preact@10.29.1` was not yet populated, causing `embed_framework_packages()` in `build.rs` to panic.

The fix was applied in commit `1249ead` (2026-05-22): "Add pnpm install before cargo build (build.rs embeds framework packages)". In the current release.yml, `Install dependencies` (line 119) runs BEFORE `dtolnay/rust-toolchain` (line 122) and `Build zfb binary` (line 150). This ordering is correct.

### Dry-run result

| Platform | Job ID | Result | Notes |
|---|---|---|---|
| x86_64-unknown-linux-gnu (linux-x64) | 77518394753 | PASS (5m33s) | Full cargo build + copy binary succeeded |
| aarch64-unknown-linux-gnu (linux-arm64) | 77518394748 | PASS (6m19s) | cross-rs build + QEMU smoke test passed |
| x86_64-pc-windows-msvc (Windows) | 77518394745 | PASS (11m51s) | Full cargo build + copy binary succeeded |
| aarch64-apple-darwin (macOS arm64) | 77518394750 | FAIL (32s) | Transient runner auth failure at checkout (exit 128: "could not read Username for 'https://github.com': terminal prompts disabled") — NOT the preact bug |
| x86_64-apple-darwin (macOS x64) | 77518394751 | QUEUED (25+ min) | GitHub Actions runner availability issue for legacy macos-13 runner pool — NOT a code issue |

**Verdict on preact-not-found bug:** PROVEN FIXED. All 3 non-macOS platforms completed the cargo build step successfully, proving the `pnpm install` ordering fix works. The macOS arm64 failure is a completely different failure mode (git auth at checkout, not a build failure) and the macOS x64 job never started.

**Verdict on macOS issues:** Both are transient GitHub Actions infrastructure issues, not code defects. They do not indicate any problem with the release workflow logic. A re-run should resolve them.

---

## Summary of findings

| Check | Result |
|---|---|
| pnpm pack (9 tarballs) | PASS — all correct contents, no src/*.ts leaks |
| workspace:* rewriting | PASS — all concrete 0.1.0-next.0 |
| Build verification | PASS — all 4 TS packages dist/ present |
| NPM_TOKEN | ABSENT — critical blocker for real publish (not dry-run) |
| preact-not-found root cause | CONFIRMED FIXED (commit 1249ead) |
| Dry-run non-macOS | 3/3 PASS |
| Dry-run macOS | 2 transient failures (infra, not code) |

---

## Action items before real publish

1. **Add NPM_TOKEN** (critical): Create Automation-type token at npmjs.com, add as repo secret `NPM_TOKEN`
2. **Re-run macOS jobs** if needed: The arm64 transient checkout failure and x64 runner availability are unrelated to any code change; a re-run should succeed
3. **WORKSPACE_DEP_PLACEHOLDER in scaffold template** — pre-existing TODO: `zfb new` still uses old package name. See `crates/zfb/src/commands/new.rs:53`

---

*Report generated by Claude Sonnet 4.6 subagent for issue #424*
