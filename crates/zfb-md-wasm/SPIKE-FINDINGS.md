# SPIKE-FINDINGS: swc_core v64 TSX pipeline on wasm32-unknown-unknown

Sub-issue #1575, epic #1572. Probed 2026-07-13.

## VERDICT: YES — compiles AND executes

`swc_core = "=64.0.0"` with **zfb-render's exact feature set** compiles green on
`wasm32-unknown-unknown` **and** the full pipeline (parse TSX → resolver →
react automatic runtime → strip TS → hygiene → fixer → codegen) **executes
correctly** as a raw wasm module under Node.js `WebAssembly` — with **zero
special configuration**: no `.cargo/config.toml`, no rustflags, no getrandom
features, no version pins beyond the workspace's existing `=64.0.0`.

None of the four expected blockers materialized (details per blocker below).

## NOTE — file location deviates from the issue spec (deliberate)

Issue #1575 asked for this file at `crates/zfb-md-wasm/SPIKE-FINDINGS.md`.
That path is a **workspace-breaker today**: the root `Cargo.toml` declares
`members = ["crates/*"]`, and a `crates/zfb-md-wasm/` directory without a
`Cargo.toml` makes **every** cargo command fail (verified empirically in this
worktree):

```
error: failed to load manifest for workspace member `.../crates/zfb-md-wasm`
referenced via `crates/*` by workspace at `.../Cargo.toml`
```

Committing a stub `Cargo.toml` to satisfy the glob would create a workspace
member, which the same issue forbids ("NOT merged as a workspace member").
So this file lives at `crates/zfb-md-wasm-SPIKE-FINDINGS.md` (a plain file
under `crates/` does not match the member glob — also verified). **Wave 2
(#1576): `git mv` this file to `crates/zfb-md-wasm/SPIKE-FINDINGS.md` when the
real crate (with its Cargo.toml) is created.**

## The probe

A throwaway crate (never committed; built under the session scratchpad at
`/private/tmp/.../scratchpad/swc-wasm-probe`, fully outside the repo tree so
workspace auto-discovery could not pick it up) depending on swc_core directly:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
swc_core = { version = "=64.0.0", features = [
    "common",
    "common_concurrent",
    "common_sourcemap",
    "ecma_ast",
    "ecma_codegen",
    "ecma_parser",
    "ecma_parser_typescript",
    "ecma_transforms",
    "ecma_transforms_react",
    "ecma_transforms_typescript",
    "ecma_visit",
] }
```

That is the feature list copied verbatim from `crates/zfb-render/Cargo.toml`.
`src/lib.rs` replicated `SwcPipeline::compile`
(`crates/zfb-render/src/swc_pipeline.rs`) verbatim minus the crate-local error
type, plus a C-ABI export (`probe_run` / `probe_output_ptr` /
`probe_output_len`) so the artifact could be executed without wasm-bindgen.

Native baseline first: `cargo test` on the probe passed (host
aarch64-apple-darwin), proving the pipeline code itself is correct before
blaming anything on wasm.

## Evidence

### Compile (Level: build, `cargo check --target wasm32-unknown-unknown`)

Toolchain: rustup `stable-aarch64-apple-darwin` = **rustc 1.96.0
(ac68faa20 2026-05-25)**; target installed job-locally via
`rustup target add wasm32-unknown-unknown` (per the epic's baked decision —
no `targets` key added to `rust-toolchain.toml`).

```
$ cargo check --target wasm32-unknown-unknown
   ...
    Checking swc_ecma_parser v39.1.1
    Checking swc_ecma_codegen v26.0.2
    Checking swc_ecma_utils v29.1.1
    Checking swc_ecma_hooks v0.7.0
    Checking swc_ecma_transforms_base v42.0.1
    Checking swc_ecma_transforms_react v46.0.1
    Checking swc_ecma_transforms_typescript v46.0.1
    Checking swc-wasm-probe v0.0.0 (.../swc-wasm-probe)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 34.67s
```

Zero errors, zero warnings, no rustflags, no `.cargo/config.toml`.
`cargo build --target wasm32-unknown-unknown --release` also green
(`Finished release profile ... in 1m 28s`).

### Execute (Level: runtime, raw `WebAssembly` in Node v24.14.0)

The release cdylib was instantiated directly — **no wasm-bindgen, no JS glue**.
The module's import list is **empty** (fully self-contained):

```
$ node run-wasm.mjs
wasm imports: []
probe_run exit code: 0
--- output ---
import { jsxs as _jsxs } from "preact/jsx-runtime";
export default function Page() {
    const n = 1;
    return /*#__PURE__*/ _jsxs("div", {
        children: [
            "hello ",
            n
        ]
    });
}
```

Input was `export default function Page(){ const n: number = 1; return
<div>hello {n}</div>; }` — automatic-runtime import synthesized, JSX
desugared, TS annotation stripped. This is the exact transform chain
zfb-render runs in production.

## Expected blockers — what actually happened

| Blocker | Outcome |
|---|---|
| `getrandom` 0.2 (`js` feature) | **Not in the graph at all.** The probe's lockfile contains zero `getrandom` entries. Nothing needed. |
| `getrandom` 0.3 (`wasm_js` backend via rustflags cfg) | **Not in the graph.** In the zfb workspace lock, getrandom 0.3.4 is pulled only by `ahash 0.8.12` — but only because native-only crates (lightningcss/dashmap under zfb-css/minify-html) enable ahash's default `runtime-rng` feature. The swc graph pulls ahash via `hashbrown 0.14`/`hstr` with default-features off (no getrandom), as the probe's isolated resolution proves. |
| `getrandom` 0.4 | **Not in the graph.** In the workspace lock, 0.4.2 comes only from `tempfile 3.27` (native fs tooling; never part of a wasm runtime graph). |
| `common_concurrent` / parking_lot on single-threaded wasm | **Compiles AND runs.** `parking_lot 0.12.5` is genuinely in the wasm-target graph (via `swc_common`), and the execution proof exercised `Lrc<SourceMap>`, `GLOBALS.set`, and the transform passes without issue. No feature trim needed. |
| `std::time::Instant` in the compile path | **Compiles; not hit at runtime on the exercised path.** On wasm32-unknown-unknown `Instant::now()` compiles but panics if called — the end-to-end execution completed with exit 0, so the parse→transform→codegen path calls no `Instant`/`SystemTime`. |

### getrandom story per major in the workspace `Cargo.lock` (for wave 2+)

- **0.2.17** — reverse-deps: `ahash 0.7.8` (← rkyv ← parcel_sourcemap ←
  lightningcss ← zfb-css/minify-html) and `ring` (← rustls ← reqwest ←
  zfb/zfb-binfetch/zfb-build). All native-only tooling; none can enter
  zfb-md-wasm's graph. If one ever does: enable the `js` cargo feature on
  getrandom 0.2 for wasm.
- **0.3.4** — reverse-dep: `ahash 0.8.12`, only with the `runtime-rng`
  feature that native-only crates enable. Cargo feature unification is
  per-invocation: `cargo build -p zfb-md-wasm --target wasm32-unknown-unknown`
  unifies features only over zfb-md-wasm's own graph, so lightningcss et al.
  cannot re-enable it. If a future dep does: getrandom 0.3's `wasm_js` backend
  needs BOTH the `wasm_js` cargo feature AND a rustflags cfg (it is not
  feature-selected) — see the ready-to-paste config below.
- **0.4.2** — reverse-dep: `tempfile 3.27.0` only. Native-only. getrandom 0.4
  keeps the same backend-cfg mechanism as 0.3.

### `.cargo/config.toml` — deliberately NOT committed

Not required: the probe is green with no rustflags, and committing a
`--cfg getrandom_backend="wasm_js"` that nothing consumes would be
speculative scaffolding. If getrandom ≥0.3 ever enters the wasm graph,
this is the known-good shape (target-scoped, so it cannot leak into native
builds or any existing CI job):

```toml
# getrandom >=0.3 selects its wasm backend via a cfg, not a cargo feature:
# https://docs.rs/getrandom/latest/getrandom/#webassembly-support
[target.wasm32-unknown-unknown]
rustflags = ['--cfg', 'getrandom_backend="wasm_js"']
```

(Plus `features = ["wasm_js"]` on the getrandom 0.3/0.4 dependency edge.)

## Working recipe for the real crate (wave 2, #1576)

1. `rustup target add wasm32-unknown-unknown` (job-local; baked decision — do
   not add a `targets` key to `rust-toolchain.toml`).
2. Depend on workspace swc_core (`swc_core = { workspace = true, features =
   [...] }`) with exactly the 11 features above. No trims, no additions
   needed for the pipeline.
3. `crate-type = ["cdylib", "rlib"]`.
4. No rustflags, no `.cargo/config.toml`, no extra pins.
5. Version drift note: the probe (own lockfile) resolved slightly newer
   patches than the workspace lock (swc_common 21.0.2 vs 21.0.1, swc_atoms
   9.0.3 vs 9.0.0, swc_ecma_parser 39.1.1 vs 39.0.2, hstr 3.0.6 vs 3.0.4).
   The umbrella pin `=64.0.0` is identical; the real crate will build against
   the workspace lock's patch set and wave-2 CI re-verifies it naturally.
6. Size data point for the epic's download-size concern: the unoptimized
   release cdylib (default profile — no LTO, no `opt-level=z`, no wasm-opt,
   no strip) is **5,502,817 bytes** (~5.5 MB, pre-gzip) for swc_core alone.
   Budget accordingly once markdown-rs + syntect join.

### Local-machine quirk (not a CI concern)

On this Mac, Homebrew's rust formula (`/opt/homebrew/bin/rustc`, 1.94.0)
shadows the rustup-managed toolchain even under `rustup run`, producing a
misleading `error[E0463]: can't find crate for 'core'` after a successful
`rustup target add`. Workaround used for all probe commands:

```sh
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

GitHub-hosted runners use standard rustup proxies, so plain
`rustup target add wasm32-unknown-unknown` + `cargo` works as planned there.

## Blind spots (what this spike did NOT prove)

- **Source-map emission at runtime**: `common_sourcemap` compiles for wasm,
  but the exercised path passes `None` to `JsWriter` (matching zfb-render's
  current production path). Runtime sourcemap generation on wasm is unproven.
- **Only one pipeline path executed**: other swc code paths (unused
  transforms, error-recovery branches) could still hit `Instant::now()` or
  other wasm-hostile calls at runtime. Compile-green covers them; execution
  covers only parse→react→strip→hygiene→fixer→codegen on a small input.
- **wasm-bindgen layer**: the probe used raw C-ABI exports. The real crate's
  wasm-bindgen surface (#1576) adds its own dependency edges — re-check the
  getrandom story when it lands (js-sys/wasm-bindgen do not pull getrandom,
  so no change expected).
- **Browsers**: execution was proven in Node v24.14.0 (V8). No browser run,
  though a zero-import wasm module has no host-API surface to differ on.
- **The full md pipeline**: markdown-rs, syntect(fancy-regex), and the
  visitor plugins are sibling/wave-2 scope; this spike is swc_core only.
