# CSS API carrier checkpoint — #4149 / #4150

Status: source reconciled; carrier acceptance **not verified**. This is the
2026-10-13 continuation checkpoint, not a release instruction or permission to
publish. The #4199 implementation manager owns execution, evidence, CI repair,
and the final merge/hold decision under
[the development policy](../../CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions)
and [planning policy](../../.claude/references/development-planning.md).

## Source and input identity

The existing `feat/4149-css-api` branch was retained. A clean, conflict-free merge
of main `183402a4afe30a68d86b894810be285c9cc54b3b` into implementation
`c4c9c4d5ffa0451c55eea597ef7254cbbb5a80b7` produced:

- Candidate source commit: `900aff3158cf12c8c1010997ad29b66044af704e`.
- Candidate source tree: `6614f97fc86022996d639fc356ca752262703b3f`.
- Original common base: `a81ad3c6a9d71def2298c77793a35b4361673016`.
- Package tuple: `@takazudo/zfb`, `@takazudo/zfb-slugify`, and all five
  `@takazudo/zfb-{darwin-arm64,darwin-x64,linux-arm64-gnu,linux-x64-gnu,win32-x64-msvc}`
  packages are **4.4.0**, imported from current main; no feature version bump.
- Addon: unpublished `zfb-css-node` 0.0.0, napi 2.16.17, napi-build 2.6.0,
  napi-derive 2.16.13, napi-sys 2.4.0. Node-API 8, `dyn-symbols`;
  `zfb` dependency has `default-features = false`.
- Addon recipe: dedicated `rustc -p zfb-css-node --lib --no-default-features
  --release --target TARGET --crate-type cdylib`; never `--workspace` for linking.
  The normal workspace builds its `rlib`. CLI builds keep normal default features.
- Linux builder: cross 0.2.5 and the unchanged `Cross.toml` images. x64 uses
  `amazonlinux:2023@sha256:5b29412077a463b4a3a8fbc99a8cdf4b929f38a3ecc8dac10328d8f36b0099b8`;
  arm64 uses
  `ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:1811a69aab16f39de09df72a991b4d7b33a0337686a827bea4890ccc399ff8f0`.
- pnpm: repository `packageManager` is **12.8.2**. SDK compatibility remains
  Node **>=22.0.0**; test the actual 22.0.0 runtime as well as the build runtime.

**The manager selected the Linux execution toolchain:** Rust 1.99.0 via
`RUSTUP_TOOLCHAIN=1.99.0`, dedicated `CARGO_HOME`/`RUSTUP_HOME` under
`/workspace/zfb-toolchain`, Node 24.19.0, corepack pnpm 12.8.2, cross 0.2.5
under `/tmp/zfb-tools`, and `CARGO_TARGET_DIR=/workspace/zfb-build/css-release`.
The first current x64 attempt below used that source/recipe tuple.
`rust-toolchain.toml` itself requests mutable `stable`; the release recipe requests
mutable Node `22` and hosted runner labels. Actual compiler verbose output,
OS/SDK/linker and Docker/image identity must accompany each executor's log. Other
platform executor tuples remain pending; historical toolchain versions do not
prove them. This continuation owner did not provision compilers or produce native
bytes; it performed the manager-authorized offline frozen pnpm setup below. Do not label an artifact row passed until its actual toolchain record and
SHA-256 values exist.

A later documentation-only commit may use this source checkpoint after proving
its diff changes no inputs. Any source, dependency, recipe, feature, package,
fixture, relevant environment or toolchain change requires a new tuple and
invalidation review before reuse. Save exact HEAD and checkout/tree identity in
every run; a branch name alone is not immutable evidence.

## Reconciliation and invalidation

Main did not change the CSS compiler seam, addon, TypeScript CSS API, CSS fixtures,
carrier helpers, or the two CSS-modified workflows. Their implementation survives
byte-for-byte. The clean merge retains both main's 4.4.0 manifests and the CSS
export/addon allowlists. No held #4196 work, image re-pin, workflow-policy change,
or unrelated branch was imported.

| Evidence at c4c9c4d5 | Current disposition and reason |
| --- | --- |
| Linux debug link/load, no-V8 graph, 41 CSS unit / 57 CLI tests | Historical pass; current native/build evidence invalidated by Cargo dependency closure: noyalib and noyalib-serde-yaml 0.0.44 → 0.0.55. CSS source identity alone does not prove equivalent linkage or configuration behavior. |
| Packed SDK/carrier, Node 22.0.0, trace, concurrency, CLI/golden and Chromium | Historical pass; rerun on newly packed 4.4.0 bytes and reconciled native graph. Old tarballs are not current evidence. |
| SDK 787 tests/typecheck and all-target workspace build | Historical pass; revalidate relevant current package/dependency/runtime input closure. |
| Docs check/build/HTML (672 pages) | Historical pass; invalidated by current docs host migration, dependencies, content and config. |
| Linux x64 release | Missing success, retaining failed pre-compilation attempt below. |
| Linux arm64 release | Missing success, retaining failed image-extraction attempt below. |
| Darwin arm64/x64, fast-Mac and Windows MSVC | Never executed in the earlier Linux session; missing, not invalidated successes. |
| CSS workflow source review / syntax | Source-identical recipe may inform review; syntax does not establish artifact execution. Current-head required CI remains missing. |

Preserve these original #4150 failures verbatim in any follow-up ledger:

- x64: `scripts/build-css-node.sh x86_64-unknown-linux-gnu cross` stopped before
  Rust compilation: pinned Dockerfile `dnf install -y -q gcc gcc-c++` returned
  `Unable to find a match: gcc gcc-c++`; guard exit 1, 10 seconds.
- arm64: the unchanged image pull failed with
  `docker: failed to register layer: no space left on device`; guard exit 125,
  37 seconds. A later retry never erases this result.

Current manager retry at source `900aff3158cf12c8c1010997ad29b66044af704e`:
Linux x64 **failed**, exit 1 after 14 seconds, before Rust compilation with the
same `Unable to find a match: gcc gcc-c++` at Dockerfile line 37. Evidence:
`/tmp/zfb-evidence/css-linux-x64-build.log`. No addon/hash was produced. The
arm64 retry in this fresh Cloud environment progressed past image extraction,
then **failed**, exit 101 after 259 seconds, because build.rs could not find
`node_modules/.pnpm/hono@4.12.25` in the worktree. This is a distinct prerequisite
failure, not another disk failure or a carrier pass. The manager then authorized
`PATH=/tmp/zfb-tools:$PATH corepack pnpm install --offline --frozen-lockfile` in
this worktree: **passed**, pnpm 12.8.2, 334 reused packages, zero downloads and no
lockfile change. The manager owns the next native retry and its separate result.
No image pin changed.

## Executor admission and shared preparation

These commands are for manager-admitted, isolated capable executors. No native
build, browser run, or pack was performed by this continuation owner. Its sole
dependency setup was the serialized offline frozen install recorded above. Use one heavy lane per host; local build commands go through
`bash "$HOME/.codex/scripts/heavy-guard.sh" -- COMMAND`. Separate hosted runners
may execute different immutable target rows concurrently. Do not share writable
node_modules, dist, targets, package binary slots, or tarball output directories.
The manager must resolve the toolchain inputs above before starting these recipes.

From a clean checkout of the selected candidate, record:

```sh
git rev-parse HEAD HEAD^{tree}
git status --porcelain
rustc --version --verbose
cargo --version
node --version
node -p 'JSON.stringify({platform:process.platform,arch:process.arch,versions:process.versions})'
pnpm --version
# Linux only:
cross --version
docker version
```

Record `uname -a` and the C/C++ linker version on Unix; `sw_vers`,
`xcodebuild -version`, `xcrun --show-sdk-version` on macOS; Windows OS build,
MSVC `cl` version and Windows SDK version on MSVC. Record relevant overrides
(`RUSTUP_TOOLCHAIN`, `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_TARGET_DIR`,
`CARGO_PROFILE_*`, `MACOSX_DEPLOYMENT_TARGET`, `SDKROOT`), without secrets. Freeze
Rust explicitly via `RUSTUP_TOOLCHAIN=<resolved version>` for subsequent commands.
Provision the selected Rust targets and pnpm 12.8.2 before the run; then:

```sh
pnpm install --frozen-lockfile
cargo tree --locked -p zfb-css-node --no-default-features -e features > css-feature-graph.txt
# Inspect this graph: neither v8 nor deno_core may occur in the dedicated addon graph.
```

The helper currently does not pass `--locked`; compare `Cargo.lock` to the frozen
hash after execution and reject changed resolution. Capture the feature graph's
SHA-256 and actual build log. Existing release recipes are reference material;
do not dispatch the publishing release workflow to obtain these checks.

## Five required carrier rows

| Carrier / Rust target | Build and execution capability | Current evidence |
| --- | --- | --- |
| linux-x64-gnu / x86_64-unknown-linux-gnu | Docker + unchanged cross recipe; x64 Node 22.0.0, native or explicitly recorded emulation; objdump GLIBC <=2.34 | Missing release artifact/load; original dnf failure retained |
| linux-arm64-gnu / aarch64-unknown-linux-gnu | Docker + unchanged cross recipe; arm64 Node 22.0.0 on arm64 host or QEMU; record which; objdump GLIBC <=2.34 | Missing release artifact/load; original disk failure retained |
| darwin-arm64 / aarch64-apple-darwin | macOS SDK/linker and arm64 Node >=22; native arm64 execution (for example a capable macos-15 runner) | Missing |
| darwin-x64 / x86_64-apple-darwin | macOS SDK/linker and x64 Node >=22; Intel native or Apple Silicon Rosetta, explicitly identified | Missing, including fast-Mac producer |
| win32-x64-msvc / x86_64-pc-windows-msvc | Windows x64 MSVC toolchain/SDK, bash and x64 Node >=22; actual Windows DLL load/compile | Missing |

For each row set `target`, `carrier`, and `builder` from that table (`cross` on
Linux, `cargo` on Darwin/Windows). From the checkout root, run through the admitted
heavy lane as applicable:

```sh
ZFB_RELEASE_VERSION=4.4.0 "$builder" build -p zfb --release --target "$target"
bash scripts/build-css-node.sh "$target" "$builder"
# On the matching native platform (x64 Node explicitly selected for Rosetta):
node scripts/smoke-css-node.mjs "packages/zfb-$carrier/zfb-css.node"
```

The helper copies the dedicated release library into the carrier as
`zfb-css.node`, mode 0644; it rejects Linux symbols above GLIBC_2.34. Also retain
`objdump -T` output and the measured maximum for each Linux **release addon**.
Run the Linux load step using the matching container architecture instead of a
wrong-architecture host Node:

```sh
# Set node_platform=linux/amd64 or linux/arm64; enable QEMU first if required.
docker run --rm --platform "$node_platform" -v "$PWD:/w:ro" -w /w \
  node:22.0.0-bookworm-slim node scripts/smoke-css-node.mjs \
  "packages/zfb-$carrier/zfb-css.node"
```

Resolve and retain that image's pulled digest/ID; the tag alone is not the frozen
runtime identity. This Bookworm load is compatibility execution, not proof of
GLIBC_2.34: the measured addon symbol ceiling is independently required. Keep the
existing CLI GLIBC guard too.

Resolve the real Cargo output directory (the helper supports redirected targets),
then stage the freshly built CLI alongside the addon. In bash on all platforms:

```sh
target_dir=$(cargo metadata --no-deps --format-version 1 | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')
binary=zfb
if [ "$carrier" = win32-x64-msvc ]; then binary=zfb.exe; fi
cp "$target_dir/$target/release/$binary" "packages/zfb-$carrier/$binary"
if [ "$binary" = zfb ]; then chmod 0755 "packages/zfb-$carrier/$binary"; fi
# Execute -V on the matching native host. For cross-built Linux use:
# docker run --rm --platform "$node_platform" -v "$PWD:/w:ro" -w /w \
#   node:22.0.0-bookworm-slim "packages/zfb-$carrier/zfb" -V
# On native Darwin/Windows:
# "packages/zfb-$carrier/$binary" -V
artifact_dir=$(mktemp -d)
(cd "packages/zfb-$carrier" && npm pack --pack-destination "$artifact_dir")
```

Inspect the packed carrier: exact package/version, correct architecture, both
CLI and addon present, CLI mode 0755 on Unix, addon bytes equal to the executed
bytes. Record SHA-256 for the native release library, staged addon, staged CLI,
carrier tarball, SDK/slugify tarballs and installer archive/checksum files. On
Windows use `Get-FileHash -Algorithm SHA256`; elsewhere `sha256sum` or
`shasum -a 256`. There are **no current artifact hashes to report** at this
checkpoint; blank artifact rows must remain missing rather than invented.

## Exact macOS-only work and portable fast-Mac work

Darwin native linking/loading and the existing fast-Mac producer need macOS.
The producer expressly rejects Linux/Windows; implementing a new Darwin SDK
cross-toolchain is not part of this task. Ordinary Darwin carrier execution may
use suitable hosted macOS runners; it does not intrinsically require the owner's
personal Mac. No Mac has been operated during this continuation.

On the admitted Mac, after selecting the frozen tuple, run the producer without
upload (the script installs dependencies and owns/restores shared binary slots):

```sh
# Apple Silicon requires a real x64 Node executable plus working Rosetta.
export ZFB_X64_NODE=/absolute/path/to/x64/node
bash "$HOME/.codex/scripts/heavy-guard.sh" -- \
  bash scripts/build-macos-x64-local.sh --out-dir /absolute/path/to/owned-output
```

Retain all four outputs, with source/toolchain metadata:

- `zfb-4.4.0-x86_64-apple-darwin.tar.gz` and `.tar.gz.sha256`.
- `zfb-css-4.4.0-x86_64-apple-darwin.node` and `.node.sha256`.

The following ingestion replay is **Linux-capable**, after those exact bytes are
transferred to a disposable directory. It mirrors the release step's independent
archive/addon verification and copying without a Release upload/download:

```sh
# Start in the disposable directory containing the four producer files.
archive=zfb-4.4.0-x86_64-apple-darwin.tar.gz
addon=zfb-css-4.4.0-x86_64-apple-darwin.node
sha256sum -c "$archive.sha256"
sha256sum -c "$addon.sha256"
test "$(tar -tzf "$archive")" = zfb
mkdir staged
# Single-file archive verified above; the addon must stay separate.
tar -C staged -xzf "$archive"
cp "$addon" staged/zfb-css.node
chmod 0755 staged/zfb
cmp "$addon" staged/zfb-css.node
sha256sum staged/zfb staged/zfb-css.node
```

Checksum fault checks can also run on Linux, in this same disposable directory:

```sh
cp "$addon" "$addon.saved"
printf corruption >> "$addon"
if sha256sum -c "$addon.sha256"; then exit 1; fi
mv "$addon.saved" "$addon"
mv "$addon" "$addon.saved"
if sha256sum -c "$addon.sha256"; then exit 1; fi
mv "$addon.saved" "$addon"
sha256sum -c "$addon.sha256"
```

Then copy `staged` bytes into an isolated candidate's darwin-x64 carrier and
`npm pack` it, verifying contents/modes and byte equality. Also replay missing
addon/checksum and mismatched-addon cases against the checked-in ingestion
step in a disposable fixture; each must fail, never fall back to an old addon.
These are one-time verification records, not newly admitted permanent tests.
Linux cannot execute the Darwin addon: the producing Mac's real x64 Node smoke
and its exact hash must accompany the replay. No release/tag creation, `--upload`,
provenance downgrade, or publishing route is authorized by this recipe.

## Existing CI routes and their limits

| Existing route at the candidate | What it can establish | Limitation/action |
| --- | --- | --- |
| `health.yml` workflow_dispatch | Current workspace/no-V8 and JS checks | Does not produce/load dedicated CSS cdylibs; manager may use for its actual scope |
| `exam.yml` workflow_dispatch, macos-exam | macOS workspace tests | Does not run the dedicated addon or fast-Mac producer; not a Darwin carrier pass |
| `island-size-darwin.yml` workflow_dispatch | Its existing Darwin size contract | Not CSS native carrier coverage |
| `node-free-smoke.yml` build-binary + scaffold-e2e | Linux release addon production/ceiling; x64 packed API/trace/browser | No workflow_dispatch; PR triggers showcase preview even for drafts, main push deploys showcase. These lifecycle actions remain held |
| `release.yml` | All five native build/stage/load recipes, Linux Node 22.0.0, separate artifact transport | Publishing workflow must not be dispatched merely for carrier checks under the accepted handoff, including its existing dry_run mode. Fast-Mac ingestion is additionally skipped by dry_run |

No existing independently dispatchable nonpublishing route covers the complete
five-carrier contract or fast-Mac ingestion. This is an execution-route gap for
the manager to coordinate with capable local/native executors, not authorization
to add a workflow, alter image pins, bypass a policy, or declare Darwin complete
from unrelated macOS CI. A mere hosted-runner label does not prove an executable
route exists for this candidate.

## Manager acceptance and issue checkpoint

After freezing the execution tuple, the manager must collect each platform row's
command, exit status, run/job URL or local log, exact SHA, target/profile,
native/emulated architecture, feature graph, toolchain/SDK/image identity, GLIBC
maximum where applicable, and the artifact hashes listed above. Retain failures,
retries, cancellations and skipped steps separately. Never call pass-on-retry a
clean pass or substitute debug output for release evidence.

Rebuild SDK/slugify once for the selected packed tuple, then run the Linux packed
consumer with the new x64 release CLI/addon (isolated writable checkout):

```sh
pnpm --filter @takazudo/zfb-slugify --filter @takazudo/zfb build
ZFB_LINUX_X64_BINARY=/absolute/path/to/frozen/zfb \
ZFB_LINUX_X64_CSS_ADDON=/absolute/path/to/frozen/zfb-css.node \
ZFB_CSS_API_BROWSER=1 \
  bash scripts/smoke-css-api-packed.sh
```

Use the manager's guarded heavy lane, pnpm 12.8.2, strace, Chromium and a user for
whom the write-denied fixture really rejects writes. The L3 packed consumer
covers declarations, lazy/root/runtime imports, CLI/API/golden parity, diagnostics,
optional-output preservation/recovery and cross-root concurrency; the native
syscall trace establishes no subprocess/config evaluation/write attempts; the
bounded L5 Chromium fixture checks actual CSS placement and reset. Retain tarball
hashes and installed-package provenance; registry 4.4.0 is not this candidate.

Also rerun affected CSS unit/command checks with their correct features, normal
workspace/no-V8 compatibility, SDK checks and current docs check/strict build/HTML
validation in the manager lane. Fresh-context review is required for gate-defining
CSS changes. Current-head required checks/approvals remain separate from local
artifact acceptance. #4150 stays open until all carrier rows pass; #4149 stays
open until its entire contract and authorized integration are complete. No
publication or downstream installed-consumer acceptance is implied.

## Frozen input SHA-256 anchors

These hashes are of the reconciled source checkpoint, not generated artifacts.
The commit/tree identifies the complete source closure; this short list makes
recipe/dependency/package drift easy to spot. The manager handoff also includes
`/tmp/zfb-evidence/css-inputs-900aff3.sha256`: 1,885 files spanning every tracked
crate/package, scripts, CSS/smoke/Wind fixtures, Docker/Cargo config, workflows
and root build/lock configuration. Its SHA-256 is
`4f7f8143d10c0154d0ca2d7d17a925835da19fc747d118510edd13e962ea6abd`.
Hashes were calculated from Git objects at the exact source commit, so concurrent
build outputs cannot contaminate them; retain this evidence file with the run logs.

```text
2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e  Cargo.lock
efb36af9090f3099d6a98d408e0c1f823cd0a1ab906665b16037f207e53ff0a7  pnpm-lock.yaml
3e18e70208ee460635e239a91c142cf67371feafb718b05617ff06f388bf96df  rust-toolchain.toml
a68ccfe122e7dd2b952e9d8f01fb7866ed59c42485213dcb88437063abd69816  Cross.toml
9812cd1d62df927d74d1ba960acc3467e3d1cb75011f9d78b77edcba522aacc0  docker/x86_64-unknown-linux-gnu.Dockerfile
fa64307d12dde162b0731ad8c25b462fd20b636ceede5f5df8c9793d0b21cada  crates/zfb-css-node/Cargo.toml
c3ec78626fe981301a8529dca5659785cb25b6e6ca4c5d41413868a005be4ab5  scripts/build-css-node.sh
dedae96f13621196b0572f4d65d1e093a0e683c04a43a551feec0c8f23fda78d  scripts/build-macos-x64-local.sh
756dfca358342710e117b7eccb4f64b717270ca1fce7b1cce61f91a1a11898b5  scripts/smoke-css-node.mjs
9f0696bd1bd08b87298b76abb382b21adcd68e1f8caca18efb5ba4fd4f1bf35b  scripts/smoke-css-api-packed.sh
1395392343ad285b32765a0b1c1cca8297f333ac5ff7af762add5b8dc0cb71ba  .github/workflows/release.yml
8bd44320bdf12f95dd177a77851ecd3858d2f039d71618667efbd3a4aa5628fb  .github/workflows/node-free-smoke.yml
98797094f1e0650e83f997b0959023fbaaea00d1f0d065262d241ad19f5f1267  packages/zfb/package.json
3a81a9957ed9430cc397db496b97f571d6e6f21d68096557b9bbbd8ce3589c34  packages/zfb-slugify/package.json
c41e7a157cac2b4fbe39b6d7df933f406883ce1dbd02b2d2ccd2a2b0e268f064  packages/zfb-darwin-arm64/package.json
769a03ba612c3c8f85028f599e0403e53366db98a86df3b06ca41d2083804f19  packages/zfb-darwin-x64/package.json
3d28c14f72a43aeed32a7421387e9b4dcb95181eb2e279b9813c4ff38ec6b571  packages/zfb-linux-arm64-gnu/package.json
29f586c8a90f44ec27f277db686b088048bd1efc8c9ae691ef868e09fce34740  packages/zfb-linux-x64-gnu/package.json
27f45b25a7815b8a947d0cde3657a38d53c24c92a735216a437f798826361861  packages/zfb-win32-x64-msvc/package.json
```
