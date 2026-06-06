# zfb-toolchain-pins

Single source of truth for pinned external tool versions (`wrangler`, `workerd`).

All crates that need to compare or display these version strings import from here rather
than duplicating them. `zfb preview` runs `pnpm exec wrangler --version` at startup and
aborts with a clear error if the reported version does not match.

## Constants

| Constant | Current value | Purpose |
| --------------------------- | --------------- | ----------------------------------------------------------------- |
| `EXPECTED_WRANGLER_VERSION` | `"4.85.0"` | Checked by `zfb preview` against the live `wrangler` CLI version |
| `EXPECTED_WORKERD_VERSION` | `"1.20260424.1"` | Documents the locked `workerd` transitive dependency |

`EXPECTED_WRANGLER_VERSION` is kept in lock-step with the exact-pinned `wrangler` entry in
the root `package.json`. `EXPECTED_WORKERD_VERSION` is not controlled directly (workerd is
a transitive dependency of wrangler); its constant here makes a single `grep` surface both
pins together.

## Bump procedure

1. Update the two constants in `src/lib.rs`.
2. Bring the root `package.json` `devDependencies` in sync (wrangler pin).
3. Run `cargo build --workspace` to catch any compile-time consumers that relied on the
   old string value.

See [CONTRIBUTING.md](../../CONTRIBUTING.md) "External tool version pins" for the full
runbook.

## Tests

```sh
cargo test -p zfb-toolchain-pins
```

This crate contains only constants — the command compiles the crate and runs zero unit
tests. The pins are exercised by consumer crates (for example, `zfb preview` checks
`wrangler --version` against `EXPECTED_WRANGLER_VERSION` at runtime).
