# Local (Mac) vs CI shipped-size measurements

`crates/zfb-md-wasm/shipped-sizes.json` holds **CI-measured values only**.
This file records what the same build produces on a Mac, so that gap stops
being re-discovered from scratch every time someone builds `zfb-md-wasm`
locally (zfb#3055, following the Mac gap eval in zfb#3049).

## 2026-09-19 measurement

- Commit: `37b3dd14f2699a98830879f9374e735d9db1c574`
- OS/arch: `Darwin arm64` (macOS, Apple Silicon)
- Rust toolchain: rustc 1.93.1 (01f6ddf75 2026-02-11), cargo 1.93.1
- wasm-bindgen: 0.2.121
- binaryen (`wasm-opt`): 130.0.0
- Node: v24.13.0
- Stamped `ZFB_RELEASE_VERSION`: `2.15.0` (matches `shipped-sizes.json`'s
  `measuredOnVersion`, so the embedded version string does not perturb the
  byte deltas below)
- Command: `CARGO_TARGET_DIR=<main-repo>/target ZFB_RELEASE_VERSION=2.15.0 pnpm test:md-wasm:local`,
  run as one command (build then test)

| Entry/graph | Column | CI (`shipped-sizes.json`) | Local (Mac) | Delta (local − CI) |
| --- | --- | ---: | ---: | ---: |
| root | final wasm | 3,399,954 B | 3,473,330 B | +73,376 B |
| root | gzip-9 | 1,516,383 B | 1,553,891 B | +37,508 B |
| root | glue | 14,998 B | 14,998 B | +0 B |
| root | glue gzip-9 | 4,199 B | 4,199 B | +0 B |
| highlight | final wasm | 1,539,334 B | 1,543,455 B | +4,121 B |
| highlight | gzip-9 | 817,951 B | 825,585 B | +7,634 B |
| highlight | glue | 8,758 B | 8,758 B | +0 B |
| highlight | glue gzip-9 | 2,637 B | 2,637 B | +0 B |
| render | final wasm | 2,196,095 B | 2,241,085 B | +44,990 B |
| render | gzip-9 | 1,091,678 B | 1,113,354 B | +21,676 B |
| render | glue | 8,772 B | 8,772 B | +0 B |
| render | glue gzip-9 | 2,661 B | 2,661 B | +0 B |
| parse | final wasm | 700,364 B | 730,338 B | +29,974 B |
| parse | gzip-9 | 283,991 B | 297,372 B | +13,381 B |
| parse | glue | 11,159 B | 11,159 B | +0 B |
| parse | glue gzip-9 | 3,797 B | 3,797 B | +0 B |

On this Mac, `render-only` gzip-9 (1,113,354 B) is 13,354 B over its
1,100,000 B ceiling. `pnpm test:md-wasm:local` downgraded that to a warning
instead of failing (see "Local workflow" below); the 219 vitest tests still
ran against the artifacts the build produced.

### Why they differ

The raw and glue-only columns come from different tools. `finalWasm`/`gzip9`
are the output of `rustc` + `wasm-bindgen` + `wasm-opt -O1`, so they reflect
platform-specific codegen (different LLVM backend build, different libc/host
allocator choices baked into the toolchain) — a Mac and a CI ubuntu runner on
the *same* pinned tool versions still emit different bytes. `glue`/`glueGzip9`
are `wasm-bindgen`'s generated JS, which is templated text independent of the
host platform, so those columns are expected to match CI exactly — and they
did, byte-for-byte, in this measurement.

### The rule: never refresh `shipped-sizes.json` from a Mac build

`shipped-sizes.json` holds CI-measured values only. `scripts/assert-zfb-md-wasm-budgets.mjs`
compares `finalWasm` and `glue` with **zero tolerance**, so writing Mac bytes
into the manifest is a guaranteed `manifest-mismatch` the next time CI runs.
When the manifest needs updating for an intentional artifact change, take the
numbers from the PR's own CI build log (the `wasm-md` job's build summary),
never from a local Mac build.

### Local workflow

`pnpm test:md-wasm` is strict by design: it fails on a Mac whenever any
artifact's gzip-9 size is over its ceiling, because the plain script has no
way to tell an intentional regression apart from this codegen gap. Use
`pnpm test:md-wasm:local` instead — it passes `--allow-over-ceiling` to the
build, which downgrades a ceiling breach to a loud warning so the build+test
can still complete locally. That opt-in flag is refused outright when `CI` is
set, so it cannot silently soften the real gate.

The opt-in used to be the `ZFB_MD_WASM_ALLOW_OVER_CEILING` env var (zfb#3054);
zfb#3060 moved it to the `--allow-over-ceiling` build argument, because an
exported env var also softened `prepublishOnly` → `pnpm build` on a
hand-rolled local `pnpm publish` (zfb#3057) — a build it was never aimed at.
The env var is now ignored outright: setting it produces a loud warning
naming `pnpm test:md-wasm:local` and has no effect on ceiling enforcement.

### `--fix` behaviour

`node scripts/assert-md-wasm-size-docs.mjs --fix` refuses to run against an
over-ceiling manifest: it reports a `manifest-over-ceiling` finding and
writes nothing. It only ever syncs doc tables from a `shipped-sizes.json`
that is already within its ceilings.

### Ceiling decision: `render-only` stays at 1,100,000 B

The current CI headroom, computed from `shipped-sizes.json`'s committed
`2.15.0` row: `1,100,000 − 1,091,678 = 8,322 B`. Raising the ceiling to fit
what a developer's Mac measures was rejected — Mac bytes never ship, so
sizing the ceiling to a machine that never produces the shipped artifact
would only hide a real regression's margin. Revisit the ceiling only when a
real, intentional change needs the room; a codegen gap on one contributor's
machine is not that trigger.

The older "up to 5,982 B" figure in `DEPENDENCIES.md` belongs to a dated
evaluation record from an earlier pin and is intentionally left as written —
it describes what that evaluation measured at the time, not the current gap.

## What was NOT tested

This is one Mac / one toolchain snapshot, not a survey. It does not cover
other macOS versions, Intel Macs, Linux, or Windows, and it does not attempt
to bound how the gap moves as the Rust toolchain, wasm-bindgen, or binaryen
are upgraded. CI-side numbers above were read from `shipped-sizes.json`, not
re-measured against a fresh CI run.
