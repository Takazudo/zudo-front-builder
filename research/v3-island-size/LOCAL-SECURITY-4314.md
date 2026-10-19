# Local Darwin collection for the #4314 security transition

This is a collection and **stage-only** handoff for #4315. It is not evidence that a Mac run has happened. Run it only after the owner authorizes collection following preflight. The measured checkout is exactly `bda633b3589e201737b02cf2060f7b24d41a45a7`; the clean, separately reviewed tooling checkout supplies the local-evidence adapter. Set `TOOL_SHA` to the full reviewed commit reported by the follow-up PR. Do not substitute the tool commit for the measured source.

This uses the existing default producer, eight fixtures, workspace and packed modes, and two saved passes per fixture. It does not run direct V8, full nextest, or native Actions. Linux native size evidence already exists separately; Linux Rust verification is also separate and cannot substitute for Darwin measurement. This transition is per-platform: a Darwin stage is not a claim that paired activation or final enforcement has completed.

## Preconditions and paths

Use native Darwin arm64 and already-installed Rust/Cargo 1.99.0, official Node 24.14.0 with zlib `1.3.1-e00f703`, and pnpm 12.8.2. The installed Hono version must be 4.12.25 and esbuild 0.25.12. No Python is required by this procedure. Do not install a global tool, edit a shell profile, or change `HOME`/`CODEX_HOME`. Missing tools or guard are a preflight blocker.

Prepare two clean checkouts containing the required Git objects: measured source at the SHA above and tooling at the reviewed `TOOL_SHA`. Keep them and all original package inputs and binaries at their recorded absolute paths through independent review and strict replay. A directory digest alone does not preserve missing original inputs. Do not move, clean, or rebuild them after collection.

Run the following blocks in one fresh Bash shell. Replace the site-specific values before running. `RETAIN` must be a newly created, permanent location outside both checkouts, not `/tmp` (a symlink on macOS). `TARGET` may be an existing compatible shared Cargo target directory; coordinate exclusive use so another build cannot replace its binary. All paths are canonicalized. The recipe uses environment changes only in this shell and its children.

```bash
set -euo pipefail
SOURCE='/absolute/path/to/clean-measured-checkout'
TOOLS='/absolute/path/to/clean-reviewed-tooling-checkout'
TOOL_SHA='REPLACE_WITH_REVIEWED_40_CHARACTER_COMMIT'
ZFB_NODE='/absolute/path/to/node-24.14.0/bin/node'
PNPM_BIN='/absolute/path/to/pnpm-12.8.2/bin'
RETAIN='/absolute/permanent/path/to/new-darwin-4314-collection'
TARGET='/absolute/path/to/compatible-cargo-target'
OPERATOR='REPLACE_WITH_OPERATOR_NAME'
SOURCE_SHA=bda633b3589e201737b02cf2060f7b24d41a45a7
GUARD="$HOME/.codex/scripts/heavy-guard.sh"

test -x "$ZFB_NODE"
test -f "$GUARD"
test ! -e "$RETAIN"
mkdir -p "$RETAIN" "$TARGET"
canonical() { (cd "$1" && pwd -P); }
SOURCE="$(canonical "$SOURCE")"
TOOLS="$(canonical "$TOOLS")"
RETAIN="$(canonical "$RETAIN")"
TARGET="$(canonical "$TARGET")"
NODE_BIN="$(canonical "$(dirname "$ZFB_NODE")")"
ZFB_NODE="$NODE_BIN/$(basename "$ZFB_NODE")"
PNPM_BIN="$(canonical "$PNPM_BIN")"
export PATH="$NODE_BIN:$PNPM_BIN:$PATH"
export RUSTUP_TOOLCHAIN=1.99.0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1
export CARGO_TARGET_DIR="$TARGET"
unset CARGO_PROFILE_DEV_DEBUG
BUNDLE="$RETAIN/bundle"
RECEIPT="$RETAIN/receipt.json"
STAGE="$RETAIN/stage"
COMMANDS="$RETAIN/collection-argv.jsonl"
export SOURCE TOOLS TOOL_SHA SOURCE_SHA BUNDLE RECEIPT COMMANDS OPERATOR
"$ZFB_NODE" --input-type=module -e '
  import {relative,isAbsolute} from "node:path";
  const {SOURCE,TOOLS,TOOL_SHA,OPERATOR}=process.env;
  const inside=(root,p)=>{const r=relative(root,p);return !isAbsolute(r)&&r!==".."&&!r.startsWith("../")};
  if (!/^[a-f0-9]{40}$/.test(TOOL_SHA) || OPERATOR.startsWith("REPLACE_")) throw Error("fill reviewed identity and operator");
  if (inside(SOURCE,TOOLS)||inside(TOOLS,SOURCE)||inside(SOURCE,process.argv[1])||inside(TOOLS,process.argv[1])) throw Error("checkouts and retained evidence must be separate");
' "$RETAIN"
for checkout in "$SOURCE" "$TOOLS"; do
  test -z "$(git -C "$checkout" status --porcelain --untracked-files=normal)"
done
test "$(git -C "$SOURCE" --no-replace-objects rev-parse HEAD)" = "$SOURCE_SHA"
test "$(git -C "$TOOLS" --no-replace-objects rev-parse HEAD)" = "$TOOL_SHA"
git -C "$SOURCE" --no-replace-objects cat-file -e 21d6807e442bbed61e578c165a91d64c4a7dc141:Cargo.lock
test "$(uname -s):$(uname -m)" = Darwin:arm64
# Verify installed Rust before invoking rustc/cargo; do not auto-install it.
rustup toolchain list | awk '{print $1}' | grep -q '^1\.99\.0-aarch64-apple-darwin$'
test "$(rustc --version | awk '{print $2}')" = 1.99.0
test "$(cargo --version | awk '{print $2}')" = 1.99.0
test "$(pnpm --version)" = 12.8.2
"$ZFB_NODE" --input-type=module -e '
  if(process.version!=="v24.14.0" || process.versions.zlib!=="1.3.1-e00f703" || process.platform!=="darwin" || process.arch!=="arm64") throw Error("wrong native Node/zlib");
'
cd "$SOURCE"
mkdir -p "$BUNDLE/logs" "$BUNDLE/pack" "$BUNDLE/packed-package" "$BUNDLE/packed-runtime" "$BUNDLE/packed-slugify"
: > "$COMMANDS"
```

Record available disk on both the target and evidence volumes, physical memory, and current memory pressure. The workflow's 8 GiB disk minimum is a refusal threshold, not a guarantee of sufficient space or RAM. On an 8 GB Mac let the shared heavy guard wait for memory and serialize work; never bypass it or add a forced timeout. If a guarded call is interrupted while queued, resume the same guarded command and retain that event. Preserve first failures; do not overwrite a failed collection directory with a retry.

```bash
df -Pk "$TARGET" "$RETAIN" > "$BUNDLE/disk-preflight.txt"
sysctl hw.memsize > "$BUNDLE/memory-preflight.txt"
memory_pressure >> "$BUNDLE/memory-preflight.txt"
free_kib="$(df -Pk "$TARGET" | awk 'NR==2 {print $4}')"
[[ "$free_kib" =~ ^[0-9]+$ ]]
free_bytes=$((free_kib * 1024))
(( free_bytes >= 8 * 1024 * 1024 * 1024 ))
profile_debug=default
if (( free_bytes < 20 * 1024 * 1024 * 1024 )); then
  export CARGO_PROFILE_DEV_DEBUG=0
  profile_debug='0 (reduced: pre-build free disk below 20 GiB)'
fi
{
  printf 'source_sha=%s\n' "$SOURCE_SHA"
  printf 'runner=%s %s\n' "$(uname -s)" "$(uname -m)"
  printf 'sw_vers=%s\n' "$(sw_vers | tr '\n' ';')"
  printf 'rustc=%s\n' "$(rustc --version --verbose | tr '\n' ';')"
  printf 'cargo=%s\n' "$(cargo --version)"
  printf 'node=%s\n' "$(node --version)"
  printf 'zlib=%s\n' "$(node -p 'process.versions.zlib')"
  printf 'pnpm=%s\n' "$(pnpm --version)"
  printf 'disk_free_before_cargo_build_bytes=%s\n' "$free_bytes"
  printf 'cargo_incremental=%s\n' "$CARGO_INCREMENTAL"
  printf 'cargo_build_jobs=%s\n' "$CARGO_BUILD_JOBS"
  printf 'cargo_profile_dev_debug=%s\n' "$profile_debug"
} > "$BUNDLE/runner-provenance.txt"

# Every collection command below is recorded as its actual argv, not a shell string.
# Individual logs and exit codes are preserved, including a failing command.
record() {
  "$ZFB_NODE" --input-type=module -e '
    import {appendFileSync} from "node:fs";
    appendFileSync(process.env.COMMANDS,JSON.stringify(process.argv.slice(1))+"\n");
  ' "$@"
}
run() {
  local label="$1"; shift
  record "$@"
  local statuses
  if "$@" 2>&1 | tee "$BUNDLE/logs/$label.log"; then
    statuses=("${PIPESTATUS[@]}")
  else
    statuses=("${PIPESTATUS[@]}")
  fi
  printf '%s\n' "${statuses[0]}" > "$BUNDLE/logs/$label.exit-code"
  (( statuses[1] == 0 )) || return 90
  return "${statuses[0]}"
}
run install pnpm install --frozen-lockfile
run cargo-build bash "$GUARD" -- cargo build --locked -p zfb
run build-slugify pnpm -C packages/zfb-slugify build
run build-zfb pnpm -C packages/zfb build
run build-runtime pnpm -C packages/zfb-runtime build
version="$(node -p "require('./packages/zfb/package.json').version")"
run pack-slugify pnpm -C packages/zfb-slugify pack --pack-destination "$BUNDLE/pack"
run pack-zfb pnpm -C packages/zfb pack --pack-destination "$BUNDLE/pack"
run pack-runtime pnpm -C packages/zfb-runtime pack --pack-destination "$BUNDLE/pack"
run extract-slugify tar -xzf "$BUNDLE/pack/takazudo-zfb-slugify-$version.tgz" -C "$BUNDLE/packed-slugify"
run extract-zfb tar -xzf "$BUNDLE/pack/takazudo-zfb-$version.tgz" -C "$BUNDLE/packed-package"
run extract-runtime tar -xzf "$BUNDLE/pack/takazudo-zfb-runtime-$version.tgz" -C "$BUNDLE/packed-runtime"
zfb="$TARGET/debug/zfb"
esbuild="$SOURCE/crates/zfb/binaries/esbuild/esbuild"
hono="$SOURCE/packages/zfb-runtime/node_modules/hono"
test -x "$zfb"; test -x "$esbuild"
test "$("$esbuild" --version)" = 0.25.12
test "$(node -p "require('./packages/zfb-runtime/node_modules/hono/package.json').version")" = 4.12.25
{
  printf 'zfb_binary='; shasum -a 256 "$zfb"
  printf 'esbuild_version=%s\n' "$("$esbuild" --version)"
  printf 'esbuild_binary='; shasum -a 256 "$esbuild"
  printf 'hono_version=%s\n' "$(node -p "require('./packages/zfb-runtime/node_modules/hono/package.json').version")"
  printf 'hono_package_json='; shasum -a 256 "$hono/package.json"
  printf 'slugify_tarball='; shasum -a 256 "$BUNDLE/pack/takazudo-zfb-slugify-$version.tgz"
  printf 'zfb_tarball='; shasum -a 256 "$BUNDLE/pack/takazudo-zfb-$version.tgz"
  printf 'runtime_tarball='; shasum -a 256 "$BUNDLE/pack/takazudo-zfb-runtime-$version.tgz"
} >> "$BUNDLE/runner-provenance.txt"
```

## Complete native collection and original checker result

These are the workflow's complete real measurement commands, using absolute retained paths and the local heavy guard. Omitting `--case` runs all eight fixtures; the runner performs and compares two full saved passes itself. Do not add a modeled-only or smoke result as a substitute.

```bash
run measure-workspace bash "$GUARD" -- "$ZFB_NODE" research/v3-island-size/measure-real.mjs \
  --mode workspace --package "$SOURCE/packages/zfb" \
  --runtime-package "$SOURCE/packages/zfb-runtime" --slugify-package "$SOURCE/packages/zfb-slugify" \
  --hono-package "$hono" --zfb-tarball "$BUNDLE/pack/takazudo-zfb-$version.tgz" \
  --runtime-tarball "$BUNDLE/pack/takazudo-zfb-runtime-$version.tgz" \
  --zfb "$zfb" --esbuild "$esbuild" --source-sha "$SOURCE_SHA" --out "$BUNDLE/real-workspace"
run measure-packed bash "$GUARD" -- "$ZFB_NODE" research/v3-island-size/measure-real.mjs \
  --mode packed --package "$BUNDLE/packed-package/package" \
  --runtime-package "$BUNDLE/packed-runtime/package" --slugify-package "$BUNDLE/packed-slugify/package" \
  --hono-package "$hono" --zfb-tarball "$BUNDLE/pack/takazudo-zfb-$version.tgz" \
  --runtime-tarball "$BUNDLE/pack/takazudo-zfb-runtime-$version.tgz" \
  --zfb "$zfb" --esbuild "$esbuild" --source-sha "$SOURCE_SHA" --out "$BUNDLE/real-packed"
run normalize-workspace "$ZFB_NODE" research/v3-island-size/normalize-identity.mjs --measurement "$BUNDLE/real-workspace/measurement.json"
run normalize-packed "$ZFB_NODE" research/v3-island-size/normalize-identity.mjs --measurement "$BUNDLE/real-packed/measurement.json"
record "$ZFB_NODE" research/v3-island-size/check-budget.mjs \
  --workspace "$BUNDLE/real-workspace/measurement.json" \
  --packed "$BUNDLE/real-packed/measurement.json" --contract research/v3-island-size/decision.json
if "$ZFB_NODE" research/v3-island-size/check-budget.mjs \
  --workspace "$BUNDLE/real-workspace/measurement.json" \
  --packed "$BUNDLE/real-packed/measurement.json" --contract research/v3-island-size/decision.json \
  > "$BUNDLE/check-budget.log" 2>&1; then
  budget_status=0
else
  budget_status=$?
fi
printf '%s\n' "$budget_status" > "$BUNDLE/check-budget.exit-code"
cat "$BUNDLE/check-budget.log"
printf 'Original checker exit: %s\n' "$budget_status"
```

The old contract still pins the old Cargo lock, so its first strict check is expected to reject that identity. Retain the exact exit code and complete log; do not label it green. Inspect every reported failure. Any additional toolchain, fixture, input, repeatability, raw or normalized-total difference is a blocker to this narrow transition, not permission to edit measurements, raise ceilings, or suppress validation. Staging independently checks the complete saved artifact, exact locks and decision, source/predecessor continuity, and unchanged totals/ceilings/zero allowance.

## Freeze the evidence, make the receipt, and stage

Before this block, independently review the original checker result and confirm the collection is complete. Keep the bundle unchanged after hashing. Do not copy `node_modules` symlinks into it: the digest rejects symlinks anywhere in the bundle or its ancestor path, special files, and an empty bundle. Receipt and stage live outside the bundle. The measured checkout may contain ignored build output but no tracked or ordinary untracked changes.

```bash
test "$(git -C "$SOURCE" --no-replace-objects rev-parse HEAD)" = "$SOURCE_SHA"
test "$(git -C "$TOOLS" --no-replace-objects rev-parse HEAD)" = "$TOOL_SHA"
test -z "$(git -C "$SOURCE" status --porcelain --untracked-files=normal)"
test -z "$(git -C "$TOOLS" status --porcelain --untracked-files=normal)"
cp "$COMMANDS" "$BUNDLE/collection-argv.jsonl"
"$ZFB_NODE" --input-type=module <<'JS'
import {readFileSync,writeFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
const {TOOLS,SOURCE_SHA,BUNDLE,RECEIPT,COMMANDS,OPERATOR}=process.env;
const {localBundleDigest,validateLocalReceipt}=await import(pathToFileURL(TOOLS+'/scripts/island-size-local-evidence.mjs'));
const receipt={schemaVersion:1,kind:'local-native-darwin',operator:OPERATOR,
  collectedAt:new Date().toISOString(),source:{commit:SOURCE_SHA,clean:true},
  platform:{os:'darwin',arch:'arm64'},
  commands:readFileSync(COMMANDS,'utf8').trim().split('\n').map(line=>JSON.parse(line)),
  bundle:{location:BUNDLE,sha256:localBundleDigest(BUNDLE)}};
validateLocalReceipt(receipt,SOURCE_SHA);
writeFileSync(RECEIPT,JSON.stringify(receipt,null,2)+'\n',{flag:'wx'});
JS
RECEIPT_SHA="$(shasum -a 256 "$RECEIPT" | awk '{print $1}')"
printf '%s  %s\n' "$RECEIPT_SHA" "$RECEIPT" > "$RETAIN/receipt.sha256"

# The stager imports `yaml` (lock-fingerprint.mjs) from the tools checkout, which
# the SOURCE install above does not provide. Install the tools checkout's own
# frozen lockfile, without lifecycle scripts; its log stays outside the bundle.
test -f "$TOOLS/pnpm-lock.yaml"
cd "$TOOLS"
if pnpm install --frozen-lockfile --ignore-scripts > "$RETAIN/tools-install.log" 2>&1; then
  tools_install_status=0
else
  tools_install_status=$?
fi
printf '%s\n' "$tools_install_status" > "$RETAIN/tools-install.exit-code"
test "$tools_install_status" = 0
test -z "$(git -C "$TOOLS" status --porcelain --untracked-files=normal)"

# Actual CLI from the reviewed tools checkout; selected source remains bda633b.
# No --apply, Actions identifiers, force switch, or live contract writes.
cd "$TOOLS"
if "$ZFB_NODE" "$TOOLS/scripts/rebaseline-island-size-darwin.mjs" \
  --source-root "$SOURCE" --tool-sha "$TOOL_SHA" --source-sha "$SOURCE_SHA" \
  --artifact "$BUNDLE" --local-receipt "$RECEIPT" --local-receipt-sha "$RECEIPT_SHA" \
  --reason cargo-producer-lock --stage "$STAGE" \
  --expected-old-cargo-sha 230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138 \
  --expected-new-cargo-sha 563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd \
  --old-cargo-ref 21d6807e442bbed61e578c165a91d64c4a7dc141:Cargo.lock \
  --added-packages rustls@0.23.46 \
  --reviewed-cargo-transition research/v3-island-size/cargo-transitions/rustls-security-4314.json \
  --previous-contract-ref "$SOURCE_SHA" > "$RETAIN/stage.log" 2>&1; then
  stage_status=0
else
  stage_status=$?
fi
printf '%s\n' "$stage_status" > "$RETAIN/stage.exit-code"
cat "$RETAIN/stage.log"
test "$stage_status" = 0
```

The receipt has exactly the fields shown: schema version, local-native kind, operator, past UTC collection timestamp, exact clean source, Darwin arm64 platform, actual command argv arrays, and retained bundle location plus inventory digest. Its SHA-256 binds the exact JSON bytes. This is operator-provided provenance and integrity, not a signed attestation or an Actions artifact. The stage additionally records both the measured source and `stagingTool` commit/root. `stagingTool` identifies the renderer/stager only; it does not claim the identity of a future apply implementation.

Return the complete original checker log/exit, measurement reports and sidecars, tarball and binary hashes, runner provenance, receipt and SHA, full stage log/exit, stage manifest and staged diffs, both exact commit identities, retained absolute locations, and any first failures or resource adaptations. Verify that both checkouts remain clean and every staged ceiling/allowance is unchanged. Retain originals, not just an archive with broken paths. Changes or disappearance of receipt, bundle, source checkout, tooling checkout, or staged bytes must be reported; the adapter revalidates retained evidence when a stage is extended or consumed.

The first collection at `TOOL_SHA` `7e79458304878d51bafd29e24ec25e93b84893ca` (2026-10-19) predates the tools install step: its stager exited 1 with `ERR_MODULE_NOT_FOUND: yaml` before reading evidence. The retained bundle and receipt were kept, the tools checkout was installed as above, and only the stage was rerun into a new directory ([#4314](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276588952)). Never rebuild, remeasure or rewrite the receipt to recover from a stager start-up failure.

Stop at independent review of the stage. No live application, merge, release, default-producer switch, additional platform collection, or final enforced success is authorized by this recipe. A successful stage establishes an auditable candidate only; subsequent application and final enforcement require their own authorization and evidence.
