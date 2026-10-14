# Proposed generated-input producer

Status: authored inputs only. No site generation or candidate execution is
claimed by this document. The manager runs the producer after the preceding
runtime gates and records its actual command, SHA, tools and exit status.

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
literal request URLs from `site/expected.json`. Decode its binary response
array only where the expected response is HTML; retain exact bytes for
`/bytes.bin`. Assert the full ordered header pairs on that route. The current
page router passes props to page components, not the incoming Request, so this
fixture intentionally covers a binary response; request-body echo remains a
separate host-bridge probe.

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
separate frozen inputs. This site alone does not claim those rows, live-host
cancellation, native graph proof, or phase-2 CLI integration.

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

Example manager commands (replace the explicit executable paths with the
actual built artifacts; record the actual build command, features and target):

```sh
python3 prototypes/direct-v8/prepare-fixtures.py \
  --zfb /absolute/current/zfb \
  --esbuild /absolute/pinned/esbuild \
  --wrapper-test-binary /absolute/current/zfb-library-test-executable \
  --rustc /absolute/current/rustc \
  --node /absolute/current/node \
  --producer-build-command 'ACTUAL MANAGER BUILD COMMAND AND FEATURE/TARGET INPUTS' \
  --output target/direct-v8-generated-inputs

python3 prototypes/direct-v8/check-generated.py \
  --snapshot target/direct-v8-generated-inputs/frozen \
  --baseline /absolute/current/direct_v8_baseline \
  --candidate /absolute/current/direct_v8 \
  --postprocessor /absolute/current/direct_v8_postprocess \
  --node /absolute/current/node \
  --chromium /absolute/already-installed/chromium \
  --output target/direct-v8-generated-results
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
`throw.ts:6:9`, with exact `sourcesContent` and the console marker. Fresh-host
cycles follow that handler failure. The plugin probe is a separate esbuild
module using prepared virtual/alias hooks and a binary request; it does not
substitute for the real CLI-generated main/inner Worker.

Only after both runtime results pass does the checker use the existing Rust
head/doctype helpers to postprocess candidate HTML, copy the frozen dist assets,
and launch one explicit installed Chromium for Counter `0 → 1` and computed
CSS. It never downloads or installs a browser. Record its actual executable
hash and version; an installed-browser observation must not be described as a
successful download of a denied pinned browser. Watchdog intervention is a
failure, and this standalone observation cannot establish candidate CLI reload
or live-host idle transport cancellation.
