# Generated-input producer and recorded observation

The manager ran the real producer after the module gate and froze 54 files at
clean commit `f2ffe216997601f5e0aa68b5625553e6700cb206`. The retained snapshot is
`target/direct-v8-generated-2/frozen`; results are in
`target/direct-v8-generated-results-1/results.json`. Compact copies of the
[manifest](evidence/generated-manifest.json) and
[results](evidence/generated-gate.json) are saved evidence. Both hosts
passed all eight cases, with seven exact response/value parity rows. The
negative case separately passed the authored source-map and console checks.

The normal CLI and wrapper test binary were built from compiler sources at
`8dcdfb35`, with default features on `x86_64-unknown-linux-gnu`, rustc 1.99.0,
`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and
`CARGO_BUILD_JOBS=4`. The recorded build sequence was `cargo build --locked -p
zfb --bin zfb`, `cargo test --locked -p zfb --lib --no-run`, then the manager's
guarded `cargo build --locked --workspace --all-targets`. Changes through the
producer commit affected prototype fixtures/harness/evidence, not compiler
sources. The manifest records this supplied provenance separately from the
measured executable hashes and commands. Actual tools were esbuild 0.25.12,
Node v24.19.0, and the normal CLI reporting the workspace placeholder
`zfb 0.0.0`; no release version was changed.

Independent read-only review verified the retained inputs and receipts before
commit. Repository hooks then reformatted the saved JSON copies; final parsed
JSON equality with the retained originals was verified. No value, input file or
runtime observation changed. The result binds the **original frozen manifest**
SHA-256 `85708be0784247c25b95bfda3eb9cb146d39e85367499b958ddecccde92e8fed`;
the original result SHA-256 is
`91b296e89f0e70a679632c571468383feff42dc69c54326281cdb1175e185a86`.
The formatted repository copies hash to
`42e53847883394f11540e94cbbf69ddc802a7660d2c99e656cd28fb5475e9976`
(manifest) and `c9fb1042b3fae07c0a79ec24d569d941fcb84517fc611453973239ddc3ced5c1`
(result). The recorded producer/checker/browser helper hashes match their
sources. This review did not rerun the engines.

`site/` is a small real ZFB project. It reuses the Counter behavior from
`tests/built-site-smoke/fixture-site/components/counter.tsx` and the
collection/`paths()`/`entry.Content` pattern from the basic-blog template. The
literal route expectations in `site/expected.json` are authored independently
of either host. The `.wasm` is an authored 39-byte module equivalent to:

```wat
(module (func (export "answer") (result i32) i32.const 42))
```

Use the manager-built normal CLI, with its current embedded SDK/runtime/esbuild,
to stage this site into an ignored directory and run a real build. The normal
CLI remains on the Deno-backed host. Example producer command after staging:

```sh
cd /absolute/path/to/ignored/direct-v8-input/site
/absolute/path/to/current/zfb build
```

Run this through the repository heavy guard. Do not generate directly into the
authored `site/`, commit generated output, set a different bundler, or use a
handwritten esbuild Worker in place of the ZFB generator. No project package
installation is needed for these SDK imports; existing CLI fixture builds use
the binary's embedded packages. If the current binary needs explicit tool
locations, record the actual overrides and their hashes rather than inferring
which tool ran.

After all producer processes exit successfully, freeze the following together:

- `.zfb-build/bundle.mjs` and its linked `.map`, plus copied `.wasm` assets in
  that bundle directory. Point both runners' `asset_root` there and load the
  unchanged module as `file:///zfb/bundle.mjs`.
- `dist/assets/` client modules and CSS, the generated HTML, and the authored
  inputs. Record every file's relative path, byte length and SHA-256, including
  alias/virtual sources or wrapper inputs added by separate prepared probes.
- The actual producer commit and dirty-state description, CLI binary hash,
  Rust/Node/esbuild versions, feature/target inputs, package/lockfile hashes,
  and command output. A dirty checkout must not be labeled as a clean commit.
- A separately emitted config bundle using the current config-loader flags and
  canonical `zfb-config-stub.mjs`: ESM, neutral platform, esnext, both existing
  config aliases. Freeze those bytes too; the CLI's temporary config bundle is
  not a persistent output to guess at after the build.

The `worker` runner protocol consumes `source`, `name`, `asset_root` and the
five literal request URLs from `site/expected.json`: JSX, MDX/content, missing
dynamic route, exported paths and consumed Wasm. Decode response bytes only
where the expected response is HTML or JSON.

The first real baseline CLI producer failed on the original `/bytes.bin` page:
the production asset pipeline reads rendered pages as UTF-8 before URL rewriting.
Its non-UTF-8 response therefore failed after rendering. Preserve that original
run under `target/direct-v8-generated-1/` and its
[failure receipt](evidence/producer-first-failure.json); the unchanged page
and an isolated reproduction input live in `negative/producer-binary/`. This is an observed
baseline production-pipeline limitation, not a candidate normalization or a
pipeline fix. The isolated minimal reproduction is authored but has not been
executed separately. The positive site omits only that unsupported production
page.
The separate esbuild-prepared plugin Worker still tests binary POST bytes and
now returns exact `[0, 255, 128, 65]` bytes, status 201 and ordered duplicate
headers from `/binary`, below the production asset-rewrite boundary.

For the single Chromium representative, write **candidate-rendered** `/` HTML
into a copy of the frozen dist tree and keep the original emitted client/CSS
bytes. Raw Worker HTML precedes the Rust renderer's head injection. Apply the
existing `zfb_build::inject_prod_head_assets` with the frozen asset URLs and the
existing doctype helpers in a separate producer/postprocessor; do not add
`zfb-build` to the candidate dependency graph or reimplement the production
pipeline in the harness. Assert the Counter SSR marker and `Count: 0`, then a
real click yielding `Count: 1`, with no page errors. The CSS marker's computed
color should be `rgb(12, 34, 56)` after the emitted stylesheet loads. These
checks share one page and one browser session.

Keep source-map failure evidence in a separate small generated negative
fixture. Its fixed authored throw location must be specified before running
either host and mapped with the existing reprojector. Do not rewrite an emitted
bundle to inject the throw or change this positive site's build to succeed
despite a rendering failure. Wrapper/inner and plugin hook fixtures remain
separate frozen inputs; both passed in the recorded run. This generated-input
gate does not establish live-host cancellation, native graph proof, or phase-2
CLI integration; the findings identify the separate evidence and boundaries.

## Executable preparation and consumption

Run only after the manager records the #4239 module gate. Prepare the normal
CLI, baseline runner, native runner, postprocessor, and zfb library test binary
in the shared guarded build lane. The wrapper producer is the explicitly
ignored verification test
`commands::dev::tests::direct_v8_export_canonical_wrapper`; it calls the real
private `wrap_dev_bundle_with_content_trace`, scans the staged site's routes,
and proves the inner bytes remain unchanged. The preparation script invokes
that already-built test executable with an exact filter; it never invokes
Cargo or duplicates the wrapper source.

The following commands replay the preparation and comparison using this
session's actual executable locations into new output directories. They do not
rebuild those binaries or assert that another checkout has the same artifacts.
On another host, select its published guard and actual tools. Rebuild and
record changed build inputs when necessary. The wrapper test executable path
comes from the `executable` field of the corresponding Cargo test artifact;
its hash suffix can change after a rebuild. The postprocessor is built with
`cargo build --locked -p zfb-build --no-default-features --example direct_v8_postprocess`.
Pass native builds through the same guard, and rebuild the candidate with the
explicit feature command in [README.md](README.md) after any workspace
all-targets build. Reuse the saved build record below only with the recorded
producer executables; after rebuilding, supply the new actual build record.

```sh
export CARGO_HOME=/workspace/zfb-cargo
export RUSTUP_HOME=/workspace/zfb-rustup
export CARGO_BUILD_JOBS=4
DIRECT_V8_GUARD=/workspace/portable-codex-home-v51/.codex/scripts/heavy-guard.sh
DIRECT_V8_BUILD_RECORD="$(python3 -c 'import json; print(json.load(open("prototypes/direct-v8/evidence/generated-manifest.json"))["producer_build_command_supplied"])')"
bash "$DIRECT_V8_GUARD" -- python3 prototypes/direct-v8/prepare-fixtures.py \
  --zfb "$PWD/target/debug/zfb" \
  --esbuild /workspace/zfb-tools/esbuild-0.25.12 \
  --wrapper-test-binary "$PWD/target/debug/deps/zfb-4e375712b1045073" \
  --rustc /workspace/zfb-cargo/bin/rustc \
  --node /opt/codex/runtimes/codex-primary-runtime/dependencies/node/bin/node \
  --producer-build-command "$DIRECT_V8_BUILD_RECORD" \
  --output target/direct-v8-generated-replay

bash "$DIRECT_V8_GUARD" -- python3 prototypes/direct-v8/check-generated.py \
  --snapshot target/direct-v8-generated-replay/frozen \
  --baseline "$PWD/target/debug/examples/direct_v8_baseline" \
  --candidate "$PWD/target/debug/examples/direct_v8" \
  --postprocessor "$PWD/target/debug/examples/direct_v8_postprocess" \
  --node /opt/codex/runtimes/codex-primary-runtime/dependencies/node/bin/node \
  --chromium /usr/bin/chromium \
  --output target/direct-v8-generated-replay-results
```

Pass the producer and browser-bearing checker through the repository heavy
guard. Both output directories must be new and below this checkout's ignored
`target/`; a failed run is retained for inspection rather than overwritten.
The preparation step records real CLI/config/negative/plugin emission commands,
logs, executable hashes and versions, git SHA and dirty inputs, and every frozen
file's length/hash. It refuses source or executable changes during preparation.
All producers must exit before `frozen/manifest.json` is written. The source
state and supplied build command are recorded separately: this does not
retroactively prove how a pre-existing executable was built.

The checker verifies the inventory before and after execution, feeds identical
JSON inputs to separate engine-receipted native processes, and records exact
response/value parity plus independently authored markers, DOM text, JSON,
status, ordered headers and binary bytes. Only DOM text assertions collapse
HTML whitespace; response bytes and parity comparisons remain exact. The
negative's generated stack is mapped by the existing Rust decoder to authored
`throw.ts:6:9`, with exact `sourcesContent` and the console marker. In the
recorded run the error wrappers
differed (`runtime error` versus `config evaluation error`); no byte-identical
error wrapper is claimed. Both had the deliberate sentinel, the authored
location, and `[error] direct-v8-negative-before-throw`. Fresh-host
cycles follow that handler failure. The plugin probe is a separate esbuild
module using prepared virtual/alias hooks, a binary POST and a second binary
response request; it does not substitute for the real CLI-generated main/inner
Worker or claim binary-page production-pipeline support.

Only after both runtime results pass does the checker use the existing Rust
head/doctype helpers to postprocess candidate HTML, copy the frozen dist assets,
and launch one explicit installed Chromium for Counter `0 → 1` and computed
CSS. It never downloads or installs a browser. Record its actual executable
hash and version; an installed-browser observation must not be described as a
successful download of a denied pinned browser. Watchdog intervention is a
failure, and this standalone observation cannot establish candidate CLI reload
or live-host idle transport cancellation.

The recorded browser was `/usr/bin/chromium`, version `154.0.8037.57`. It
observed `Count: 0`, then `Count: 1` after a real click, and computed CSS
`rgb(12, 34, 56)`, with empty page-error and console-error arrays and exit 0.
The stylesheet was `/assets/styles-1f03d86f.css` and the island module was
`/assets/islands-46342adf.js`. This used the existing installed browser after
the earlier pinned Playwright browser download was denied with HTTP 403; the
denied download was not retried or represented as successful.
