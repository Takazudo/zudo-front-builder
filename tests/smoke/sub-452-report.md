# Sub #452 — Wave 2 Smoke Confirmation Report

**Date:** 2026-05-24
**Branch:** next4-release-fixes/sub-452-smoke-confirm
**Base:** base/next4-release-fixes (Wave 1 merged: #447, #448, #449, #450, #451)
**Platform:** linux-x64-gnu

## Summary

All 9 checks PASS. Wave 1 fixes are confirmed working end-to-end.

## Check Table

| # | Check | Sub | Result | Evidence |
|---|-------|-----|--------|----------|
| 1 | `pnpm install && pnpm -r build` | — | ✅ PASS | All 4 TS packages built; docs (Astro) built; no errors |
| 2 | Cargo build with `ZFB_RELEASE_VERSION=0.1.0-next.4` | #448 | ✅ PASS | `zfb --version` → `zfb 0.1.0-next.4`; binary at `/home/takazudo/.cargo-target/release/zfb`, placed at `packages/zfb-linux-x64-gnu/zfb` with `chmod +x` |
| 3a | `npm pack --dry-run --json` mode for `zfb` binary | #447 | ✅ PASS | `jq '.[0].files[] \| select(.path == "zfb") \| .mode'` → **493** (0755) |
| 3b | `pnpm pack` + `tar tzvf` mode for `zfb` binary | #447 (sanity) | ✅ EXPECTED | `tar tzvf` shows `-rw-r--r--` (0644) — this is the known pnpm limitation that Sub #447 fixed by switching to `npm publish` |
| 3c | `npm publish --dry-run --tag next` mode for `zfb` binary | #447 | ✅ PASS | JSON output shows `"path": "zfb", "mode": 493` — load-bearing publish check GREEN |
| 4a | Consumer: `zfb --version` → `zfb 0.1.0-next.4` | #448 | ✅ PASS | Consumer installed from local tarballs; binary reports correct version |
| 4b | Consumer: `zfb --help` without EACCES | #447 | ✅ PASS | `--help` output printed cleanly; no EACCES; binary was 0755 in npm-packed tarball |
| 5 | `zfb build` with `tsconfig.json` `"paths": {"@/*": ["src/*"]}` | #450 | ✅ PASS | `✓ 1 pages built in 0.29s`; `@/components/Greeting` resolved; no "Could not resolve" errors |
| 6 | `zfb build` with content collection + `getCollection()` | #449 | ✅ PASS | `✓ 1 pages built in 0.29s`; no `cannot load node:fs / node:path` error |
| 7 | Launcher EACCES detection: 0644 fake binary → stderr message + path | #447 | ✅ PASS | Stderr: `[zfb] binary is not executable; was the install corrupt?\n      /tmp/zfb-eacces-test/.../zfb` |
| 8 | `sync-platform-versions.mjs` rewrites `WORKSPACE_DEP_PLACEHOLDER` | #451 | ✅ PASS | Bumped `packages/zfb/package.json` to `0.99.0-smoketest`, ran script → `new.rs` shows `=0.99.0-smoketest`; all 7 packages updated; restored to `0.1.0-next.3` |
| 9 | `pnpm-lock.yaml` diff — only version-string changes, no surprise deps | — | ✅ PASS | `diff <(git show main:pnpm-lock.yaml) <(git show HEAD:pnpm-lock.yaml)` → **empty** (identical); Wave 1 fixes do not touch the npm dependency graph |

## Key Observations

### Sub #447 (binary chmod + launcher EACCES)

- `npm pack --dry-run --json` → mode `493` (0755) ✅
- `pnpm pack` → mode `0644` as expected (this is WHY Sub #447 switches to `npm publish`)
- `npm publish --dry-run --tag next` → mode `493` (0755) ✅ — the load-bearing check
- EACCES branch in `bin/zfb.mjs` triggers correctly when binary is 0644

### Sub #448 (--version stamping)

- `option_env!("ZFB_RELEASE_VERSION")` baked at compile time in `crates/zfb/src/cli.rs`
- Built with `ZFB_RELEASE_VERSION=0.1.0-next.4` → `zfb --version` reports `zfb 0.1.0-next.4` ✅
- Note: npm package version stays at `0.1.0-next.3` (Sub #453 does the real bump)

### Sub #449 (paths() snapshot flow via globalThis)

- `getCollection()` call in `getStaticProps()` works without `cannot load node:fs` ✅
- Fix: `setContentSnapshot()` stores on `globalThis.__zfb.contentSnapshot` so both module instances share state

### Sub #450 (tsconfig @/ alias regression fix)

- `"paths": {"@/*": ["src/*"]}` in `tsconfig.json` → `@/components/Greeting` resolves ✅
- Fix: `--preserve-symlinks` is now gated on `node_modules_preserve_symlinks: false` (default)

### Sub #451 (sync-platform-versions rewrites WORKSPACE_DEP_PLACEHOLDER)

- Script rewrites all 7 workspace package versions and `new.rs` const atomically ✅
- Exact-pin format (`=0.99.0-smoketest`) confirmed correct

## Wave 2 Sign-Off

**All 5 Wave 1 fixes confirmed. Ready to proceed to Sub #453 (version bump).**
