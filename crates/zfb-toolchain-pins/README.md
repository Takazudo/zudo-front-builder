# zfb-toolchain-pins — EXPECTED_WRANGLER_VERSION, EXPECTED_WORKERD_VERSION

Single source of truth for pinned external tool versions (`wrangler`,
`workerd`).

All crates that need to compare or display these version strings import from here rather
than duplicating them. `zfb preview` runs `pnpm exec wrangler --version` at
startup and treats the wrangler constant as the **minimum supported version**:
older aborts with upgrade guidance, equal passes silently, newer proceeds with
an info/warning line (issue 2379). Bundling is native (Rolldown, in process),
so no bundler executable is pinned here.

## Constants

| Constant | Current value | Purpose |
| --------------------------- | --------------- | ----------------------------------------------------------------- |
| `EXPECTED_WRANGLER_VERSION` | `"4.85.0"` | Minimum supported `wrangler` version, gated by `zfb preview` against the live CLI |
| `EXPECTED_WORKERD_VERSION` | `"1.20260424.1"` | Documents the locked `workerd` transitive dependency |

`EXPECTED_WRANGLER_VERSION` is kept in lock-step with the exact-pinned `wrangler` entry in
the root `package.json`. `EXPECTED_WORKERD_VERSION` is not controlled directly (workerd is
a transitive dependency of wrangler); its constant here makes a single `grep` surface both
pins together.

## Bump procedure

1. Update the relevant constants in `src/lib.rs`.
2. For a wrangler bump, bring the root `package.json` `devDependencies` and
   lockfile in sync; confirm the resolved `workerd` version and update
   `EXPECTED_WORKERD_VERSION` if it changed.
3. Run `cargo build --workspace` to catch any compile-time consumers that
   relied on the old string value.

See [CONTRIBUTING.md](../../CONTRIBUTING.md) "External tool version pins" for the full
runbook.

## Tests

```sh
cargo test -p zfb-toolchain-pins
```

This crate contains only constants — the command compiles the crate and runs
zero unit tests. The pins are exercised by consumer crates: `zfb preview`
gates `wrangler --version` on `EXPECTED_WRANGLER_VERSION` as the minimum
supported version.
