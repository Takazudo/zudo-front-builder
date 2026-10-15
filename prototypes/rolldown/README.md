# Internal direct-Rust Rolldown feasibility probe (#318)

Direction: **experimental opt-in for dogfooding**, subject to the whole-PR integration gates. PR #4120 remains draft until its owner reviews those gates; successful prototype checks alone do not authorize merging. The pinned Rust API can produce executable ZFB browser and embedded-V8 SSR bundles with authoritative graph/output provenance. The standalone client-script entry and its declared module workers run through the same opt-in. These fixtures do not justify a default switch or a speed claim. See [FINDINGS.md](FINDINGS.md) for the evidence and ownership costs.

The opt-in is deliberately internal: compile `rolldown-prototype` on `zfb-build` and `zfb-islands` (or the forwarding `zfb` CLI feature), then set `ZFB_ROLLDOWN_PROTOTYPE=1`. Without both, the existing esbuild path runs. The existing `NativeRustBundler` public placeholder is unchanged. No Rolldown types cross the new crate boundary.

The shared browser/SSR adapter generates in memory, validates warnings and graph
provenance, and publishes after its scoped worker/runtime has joined. It remains
synchronous and does not acquire the standalone plugin probe's supervisor,
admission or request cancellation. The real
plugin/config callers remain on esbuild, and hard native termination remains open.

Raw CSS bundling is deliberately unsupported by this experimental backend at
its unchanged native pin. A real CSS loader fails with
`ZFB_ROLLDOWN_RAW_CSS_UNSUPPORTED` before any output is published. Explicit text
and file resources remain supported, and ordinary esbuild CSS behavior is unchanged.
Selected trailing-directory tsconfig mappings combined with nonempty `rootDirs`
are rejected with `ZFB_ROLLDOWN_TRAILING_ROOT_DIRS_UNSUPPORTED`; the prototype does
not permit that native fallback to escape the prepared shadow.

## Whole-PR integration verification

See [the current integration review](INTEGRATION-20261015.md) for lock/provenance
and size blockers, source identities and merge-triggered workflow effects.
After installing the pinned tools and building the local SDK packages, run
`bash prototypes/rolldown/verify-integration.sh` through the shared heavy guard.
It checks feature-absent execution, compiled-but-unselected execution, explicit
native selection, the no-V8 compile boundary, Wind freshness and the existing
prepared/browser/SSR/production/client-worker harnesses. Reports and dependency
graphs are written under `target/rolldown-integration-evidence`. The read-only
exact-SHA workflow runs this alongside the plugin regressions; ordinary required
CI and island provenance remain separate gates.

The browser probes normally use Playwright's pinned Chromium. An explicitly
supplied `ZFB_CHROMIUM_EXECUTABLE=/absolute/path/to/chromium` can use an installed
browser when downloads are unavailable; both probes print its actual version.
Record that environment separately from CI, without claiming platform equivalence.

## Reproduce (Linux x86_64)

Run from the repository root. Rust/Cargo must be available; the recorded run used rustc 1.99.0 and Node 24.19.0. Browser fixture preparation uses local copies of the repository SDK and a pinned Hono package. Nothing is published.

```sh
export ZFB_PROTOTYPE_TOOLS="$PWD/target/rolldown-tools"
npm install --prefix "$ZFB_PROTOTYPE_TOOLS" --no-audit --no-fund hono@4.12.8 @playwright/test@1.58.2 @esbuild/linux-x64@0.25.12
export ZFB_ESBUILD_BIN="$ZFB_PROTOTYPE_TOOLS/node_modules/@esbuild/linux-x64/bin/esbuild"
export PLAYWRIGHT_BROWSERS_PATH="$PWD/target/rolldown-browsers"
"$ZFB_PROTOTYPE_TOOLS/node_modules/.bin/playwright" install chromium
node prototypes/rolldown/prepare.mjs
export CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0
bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo build --locked -p zfb-build -p zfb-islands --features rolldown-prototype --example rolldown_ssr --example rolldown_browser
bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo build --locked -p zfb-rolldown-prototype --features native --example prepared
node prototypes/rolldown/gate.mjs
```

Use the shared guard where installed, as required in this managed session; an ordinary external checkout can run the same Cargo/browser commands directly. The initial Cargo Git fetch also fetches upstream's test submodules. The lockfile and exact Git revision are necessary: Rust crates are not a stable supported API.

Run the already built binaries, keeping compilation outside measured samples:

```sh
unset ZFB_ROLLDOWN_PROTOTYPE
ZFB_DEV_TIMING=1 target/debug/examples/rolldown_ssr target/rolldown-prototype/project
ZFB_DEV_TIMING=1 target/debug/examples/rolldown_browser target/rolldown-prototype/project
ZFB_ROLLDOWN_PROTOTYPE=1 ZFB_DEV_TIMING=1 target/debug/examples/rolldown_ssr target/rolldown-prototype/project
ZFB_ROLLDOWN_PROTOTYPE=1 ZFB_DEV_TIMING=1 target/debug/examples/rolldown_browser target/rolldown-prototype/project
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node prototypes/rolldown/browser.mjs target/rolldown-prototype/project
```

Each producer runs three repetitions and asserts deterministic bytes within its backend; cross-backend byte identity is not required. SSR renders real HTML in the existing V8 host. Browser production publication returns the exact hashed entry URL in `target/rolldown-prototype/browser-{backend}.json`; the browser uses that receipt, never a directory's first matching filename. Files remain under ignored `target/`.

A single guarded prepared-binary run also checks a second fresh client stage and executes Chromium:

```sh
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node prototypes/rolldown/verify.mjs
```

Selected existing fixtures (the first six tests were observed on both backends; stage escape and the two exact-alias tests were observed with native selected; all commands can be compared on either backend):

```sh
cargo test --locked -p zfb-build --features rolldown-prototype --test bundler_css_modules --test bundler_workspace_pkg_alias --test bundler_main_fields
cargo test --locked -p zfb-build --features rolldown-prototype --test bundler_root_workspace_stage_escape_audit_armed_regression -- --ignored
cargo test --locked -p zfb-build --features rolldown-prototype --test bundler_exact_match_resolution plugin_alias_matches_exact_specifier
cargo test --locked -p zfb-build --features rolldown-prototype --test bundler_exact_match_resolution plugin_alias_does_not_match_prefix_with_slash
cargo check --locked -p zfb-build -p zfb-islands --no-default-features
cargo fmt --all --check
```

Keep `ZFB_ESBUILD_BIN` exported so the existing fixture discovery cannot skip. Feature-off checking disables the native dependency and V8 for this bounded compile check; baseline runtime above executes the feature-enabled binary with native selection off and V8 on.

## Real production second pass

The bounded mixed SSG/SSR catch-all fixture now runs the actual CLI production rebundle and local adapter, then executes scratch and adapter output in existing V8. See [PRODUCTION.md](PRODUCTION.md) for assertions, exact invocation, adjacent-pin rehearsal, manual rollback, and the baseline SSR copied-glue limitation. No deployment occurs.

## Standalone client-script harness (new, 2026-10)

From the repository root, prepare the feature-enabled `target/debug/zfb` CLI (or select an already built release binary with `ZFB_PROTOTYPE_PROFILE=release`). Set `ZFB_PROTOTYPE_TOOLS` to the tools directory described above, install Playwright Chromium there, and set `ZFB_ESBUILD_BIN` to the absolute pinned esbuild binary for this platform. On Darwin arm64 the package is `@esbuild/darwin-arm64@0.25.12`. Then run:

```sh
node prototypes/rolldown/gate.mjs
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node prototypes/rolldown/client-scripts.mjs
```

The harness copies [client-fixture/](client-fixture/) into ignored `target/rolldown-client-scripts/<backend>/` paths, runs real `zfb build` and `zfb dev` commands for both backends, executes their client entry and nested module workers in Chromium, checks `ZFB_DEV_TIMING=1` per-job attribution, performs one watched worker-dependency edit, and checks missing-import and worker stage-escape CLI failures. The native report requires one entry and two worker jobs in both production and the dev edit; the esbuild report requires zero native jobs. Each backend writes `target/rolldown-client-scripts/<backend>/evidence/report.json`. This is a correctness probe, not a new benchmark or part of the historical measurement set. The observed local run used Darwin arm64 and the debug profile; Linux native client-script execution and Intel remain unverified.

## Release comparison and maintenance checks

[RELEASE-MEASUREMENT.md](RELEASE-MEASUREMENT.md) provides the bounded optimized-build comparison, fail-closed copy-provenance checks, and deferred Mac issues. `ZFB_PROTOTYPE_PROFILE=release` selects already-built release probes in `verify.mjs` and `production.mjs`; it does not change backend selection or product defaults.
