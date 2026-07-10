# zfb-toolchain-pins — EXPECTED_WRANGLER_VERSION, EXPECTED_WORKERD_VERSION, EXPECTED_ESBUILD_VERSION

Single source of truth for pinned external tool versions (`wrangler`,
`workerd`, `esbuild`).

All crates that need to compare or display these version strings import from here rather
than duplicating them. `zfb preview` runs `pnpm exec wrangler --version` at
startup and aborts with a clear error if the reported version does not match;
`zfb-islands` and `crates/zfb/build.rs` import the esbuild version pin from
this crate.

## Constants

| Constant | Current value | Purpose |
| --------------------------- | --------------- | ----------------------------------------------------------------- |
| `EXPECTED_WRANGLER_VERSION` | `"4.85.0"` | Checked by `zfb preview` against the live `wrangler` CLI version |
| `EXPECTED_WORKERD_VERSION` | `"1.20260424.1"` | Documents the locked `workerd` transitive dependency |
| `EXPECTED_ESBUILD_VERSION` | `"0.25.12"` | Used by `zfb-islands` version checks and by `crates/zfb/build.rs` download URLs |

`EXPECTED_WRANGLER_VERSION` is kept in lock-step with the exact-pinned `wrangler` entry in
the root `package.json`. `EXPECTED_WORKERD_VERSION` is not controlled directly (workerd is
a transitive dependency of wrangler); its constant here makes a single `grep` surface both
pins together. `EXPECTED_ESBUILD_VERSION` is the esbuild version source of
truth; `crates/zfb-islands/src/esbuild.rs` re-exports it for callers that
previously imported the pin from the islands crate.

## Bump procedure

1. Update the relevant constants in `src/lib.rs`.
2. For a wrangler bump, bring the root `package.json` `devDependencies` and
   lockfile in sync; confirm the resolved `workerd` version and update
   `EXPECTED_WORKERD_VERSION` if it changed.
3. For an esbuild bump, update `EXPECTED_ESBUILD_VERSION` here,
   `EXPECTED_ESBUILD_SHA256` in `crates/zfb-islands/src/esbuild.rs`, and the
   platform-specific esbuild SHA-256 table in `crates/zfb/build.rs` in the
   same commit.
4. Run `cargo build --workspace` to catch any compile-time consumers that
   relied on the old string value.

See [CONTRIBUTING.md](../../CONTRIBUTING.md) "External tool version pins" for the full
runbook.

## Tests

```sh
cargo test -p zfb-toolchain-pins
```

This crate contains only constants — the command compiles the crate and runs
zero unit tests. The pins are exercised by consumer crates: `zfb preview`
checks `wrangler --version` against `EXPECTED_WRANGLER_VERSION`, while
`zfb-islands` checks the esbuild subprocess version against
`EXPECTED_ESBUILD_VERSION`.
