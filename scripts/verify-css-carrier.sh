#!/usr/bin/env bash
set -euo pipefail
umask 022
# Hosted verification only. Never invoke a publisher or build on a user's Mac.
mode="${1:?native|fast-mac|ingestion|linux-size}"
export CSS_VERIFY_MODE="$mode"
# The lockstep packages share one version; verify the source's own, never a pinned literal (#4415).
CSS_PACKAGE_VERSION=$(node -p 'require("./packages/zfb/package.json").version')
export CSS_PACKAGE_VERSION
if [[ "$mode" == native || "$mode" == fast-mac ]]; then
  export ZFB_RELEASE_VERSION="$CSS_PACKAGE_VERSION"
elif [[ ${ZFB_RELEASE_VERSION+x} ]]; then
  echo 'Non-release verification must retain the unstamped health environment' >&2
  exit 1
fi
tooling="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$RUNNER_TEMP/css-carrier-${CSS_EVIDENCE_ID:?}"
mkdir -p "$out/pack"
if [[ "$RUNNER_OS" == Windows ]]; then py=python; else py=python3; fi
check() { "$py" "$tooling/css-carrier-verification.py" "$@"; }
capture_cross() {
  local phase="$1" required="${2:-false}" image="" origin=selected-cross-builder
  {
    cross --version
    docker image ls --digests --no-trunc
    if [[ "$CSS_TARGET" == aarch64-unknown-linux-gnu ]]; then
      image=$(sed -n 's/^image = "\(.*\)"$/\1/p' Cross.toml)
    else
      image=$(docker image ls --format '{{.Repository}}:{{.Tag}}' | awk '/x86_64-unknown-linux-gnu/ {print; count++} END {if (count > 1) exit 1}')
      if [[ -z "$image" ]]; then
        image=$(awk '$1 == "FROM" {print $2}' docker/x86_64-unknown-linux-gnu.Dockerfile)
        origin=base-only-no-completed-cross-builder
      fi
    fi
    printf 'image_origin=%s\nimage_ref=%s\n' "$origin" "$image"
    if ! docker image inspect --format '{{.Id}} {{json .RepoDigests}}' "$image"; then
      echo 'image_identity=unavailable'; [[ "$required" != true ]]; return
    fi
    local compiler=cc linker=ld
    if [[ "$CSS_TARGET" == aarch64-unknown-linux-gnu ]]; then compiler=aarch64-linux-gnu-gcc; linker=aarch64-linux-gnu-ld; fi
    docker run --rm --entrypoint sh "$image" -c 'cat /etc/os-release; "$1" --version; "$2" --version' sh "$compiler" "$linker"
    if [[ "$required" == true ]]; then test "$origin" = selected-cross-builder; fi
  } >> "$out/cross-$phase.txt" 2>&1
}
finish() {
  status=$?
  set +e
  if [[ "$mode" == native && "${CSS_BUILDER:-}" == cross ]]; then capture_cross final false; fi
  printf '{"exitCode":%s,"mode":"%s"}\n' "$status" "$mode" > "$out/status.json"
  exit "$status"
}
trap finish EXIT
check snapshot "$out/source-files.json"
test "$(git rev-parse HEAD)" = "$SOURCE_SHA"
test "$(git -C "$tooling/.." rev-parse HEAD)" = "$WORKFLOW_SHA"
node -e 'if (process.version !== "v24.14.0") throw Error("build/packing requires Node24.14.0")'
test "$(pnpm --version)" = 12.8.2
node --input-type=module - "$out" "$tooling" <<'JS'
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
const [out, tooling] = process.argv.slice(2);
for (const name of ['zfb', 'zfb-slugify', 'zfb-darwin-arm64', 'zfb-darwin-x64', 'zfb-linux-arm64-gnu', 'zfb-linux-x64-gnu', 'zfb-win32-x64-msvc']) {
  if (JSON.parse(fs.readFileSync(`packages/${name}/package.json`)).version !== process.env.CSS_PACKAGE_VERSION) throw Error(`${name} must be ${process.env.CSS_PACKAGE_VERSION}`);
}
const sha = path => crypto.createHash('sha256').update(fs.readFileSync(path)).digest('hex');
fs.writeFileSync(`${out}/identity.json`, JSON.stringify({
  sourceSHA: execFileSync('git', ['rev-parse','HEAD'], {encoding:'utf8'}).trim(),
  sourceTree: execFileSync('git', ['rev-parse','HEAD^{tree}'], {encoding:'utf8'}).trim(),
  workflowSHA: process.env.WORKFLOW_SHA, workflowRef: process.env.WORKFLOW_REF,
  runId: process.env.GITHUB_RUN_ID, runAttempt: process.env.GITHUB_RUN_ATTEMPT,
  target: process.env.CSS_TARGET ?? null, carrier: process.env.CSS_CARRIER ?? null,
  profile: process.env.CSS_VERIFY_MODE === 'linux-size' ? 'debug' : process.env.CSS_VERIFY_MODE === 'ingestion' ? 'not-built' : 'release',
  features: process.env.CSS_VERIFY_MODE === 'linux-size' ? 'workspace defaults; all-targets; existing health size recipe' : process.env.CSS_VERIFY_MODE === 'ingestion' ? 'received native tarballs; no compilation' : 'CLI defaults; addon no-default-features; cdylib; dedicated graph',
  packageVersion:process.env.CSS_PACKAGE_VERSION,
  node:process.version, npm:execFileSync('npm',['--version'],{encoding:'utf8',shell:process.platform==='win32'}).trim(),
  platform:process.platform, arch:process.arch, python:execFileSync(process.env.RUNNER_OS==='Windows'?'python':'python3',['--version'],{encoding:'utf8'}).trim(),
  helperHashes:Object.fromEntries(['verify-css-carrier.sh','css-carrier-verification.py','css-carrier-public-smoke.mjs'].map(name => [name,sha(`${tooling}/${name}`)])),
  overrides:Object.fromEntries(Object.entries(process.env).filter(([name]) => (/^(?:CARGO_|CROSS_|RUST|ZFB_|CSS_)/.test(name) || ['SDKROOT','MACOSX_DEPLOYMENT_TARGET','CC','CXX','CFLAGS','CXXFLAGS','LDFLAGS'].includes(name)) && !/TOKEN|SECRET|PASSWORD/.test(name))),
},null,2)+'\n');
JS
{
  uname -a
  node --version; npm --version; pnpm --version
  if [[ "$RUNNER_OS" == macOS ]]; then
    sw_vers; xcodebuild -version; xcrun --show-sdk-path; xcrun --show-sdk-version; xcrun clang --version
  elif [[ "$RUNNER_OS" == Windows ]]; then
    cmd //c ver
    cat > "$out/windows-toolchain.ps1" <<'PS'
$ErrorActionPreference = 'Stop'
Get-CimInstance Win32_OperatingSystem | Format-List Caption,Version,OSArchitecture
$vs = & "${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe" -latest -property installationPath
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($vs)) { throw 'Cannot discover Visual Studio' }
$devcmd = Join-Path $vs 'Common7/Tools/VsDevCmd.bat'
if (-not (Test-Path $devcmd)) { throw 'Missing Visual Studio developer environment' }
$record = @"
@echo off
call "$devcmd" -arch=x64 -host_arch=x64 >nul
if errorlevel 1 exit /b 1
if not defined VCToolsVersion exit /b 1
if not defined VCToolsInstallDir exit /b 1
if not defined WindowsSdkDir exit /b 1
if not defined WindowsSDKVersion exit /b 1
echo discovery=default VS developer environment; no build recipe override
echo VCToolsVersion=%VCToolsVersion%
echo VCToolsInstallDir=%VCToolsInstallDir%
echo WindowsSdkDir=%WindowsSdkDir%
echo WindowsSDKVersion=%WindowsSDKVersion%
where cl >nul
if errorlevel 1 exit /b 1
where link >nul
if errorlevel 1 exit /b 1
set "CSS_COMPILER_PATH="
set "CSS_LINKER_PATH="
for /f "delims=" %%I in ('where cl') do if not defined CSS_COMPILER_PATH set "CSS_COMPILER_PATH=%%I"
for /f "delims=" %%I in ('where link') do if not defined CSS_LINKER_PATH set "CSS_LINKER_PATH=%%I"
echo compilerPath=%CSS_COMPILER_PATH%
echo linkerPath=%CSS_LINKER_PATH%
cl /Bv 2>&1
echo compiler_version_probe_exit=%ERRORLEVEL% (no source compilation requested)
link /? 2>&1
exit /b 0
"@
$path = Join-Path $PSScriptRoot 'windows-toolchain.cmd'
[System.IO.File]::WriteAllText($path, ($record -replace '\r?\n', "`r`n"))
& cmd /d /c $path
if ($LASTEXITCODE -ne 0) { throw 'Cannot record default-selected MSVC/SDK environment' }
PS
    set +e
    powershell -NoProfile -NonInteractive -File "$(cygpath -w "$out/windows-toolchain.ps1")" > "$out/windows-toolchain.txt" 2>&1
    windows_status=$?
    set -e
    cat "$out/windows-toolchain.txt"
    if [[ "$windows_status" != 0 ]]; then echo 'Windows toolchain capture process failed' >&2; exit "$windows_status"; fi
    check windows-toolchain "$out/windows-toolchain.txt" "$out/windows-toolchain.json"
  else
    cat /etc/os-release; cc --version; ld --version
  fi
  if [[ "$mode" != ingestion ]]; then
    rustc --version --verbose; cargo --version; rustup show
    test "$(rustc --version | awk '{print $2}')" = 1.99.0
  fi
} > "$out/toolchain.txt" 2>&1
if [[ "$RUNNER_OS" == Windows && -n "${ZFB_LOAD_NODE:-}" ]]; then ZFB_LOAD_NODE=$(cygpath -u "$ZFB_LOAD_NODE"); fi
run() {
  local name="$1"; shift
  set +e
  "$@" 2>&1 | tee "$out/$name.log"
  local status="${PIPESTATUS[0]}"
  set -e
  printf '%s\t%s\n' "$name" "$status" >> "$out/command-status.tsv"
  check assert-source "$out/source-files.json"
  return "$status"
}
graph() {
  cargo tree --locked -p zfb-css-node --no-default-features --target all --edges normal,build,features --prefix none > "$out/addon-feature-graph.txt"
  if grep -Eq '^(v8|deno_core) v[0-9]' "$out/addon-feature-graph.txt"; then
    echo 'V8/deno_core in dedicated addon graph' >&2; return 1
  fi
  check assert-source "$out/source-files.json"
}
pack_sdk() {
  run slugify-build pnpm -C packages/zfb-slugify build
  run sdk-build pnpm -C packages/zfb build
  run slugify-pack pnpm -C packages/zfb-slugify pack --pack-destination "$out/pack"
  run sdk-pack pnpm -C packages/zfb pack --pack-destination "$out/pack"
}
pack_carrier() {
  local package="$1" binary="$2" addon="$3"
  run carrier-pack npm pack "$package" --ignore-scripts --pack-destination "$out/pack"
  check pack "$out/pack/takazudo-zfb-${CSS_CARRIER}-${CSS_PACKAGE_VERSION}.tgz" "$CSS_CARRIER" "$binary" "$addon" "$out/packed-carrier.json"
}
load_consumer() {
  local binary="$1" addon="$2"
  export CSS_ADDON_SHA
  CSS_ADDON_SHA=$(node -e 'process.stdout.write(require("node:crypto").createHash("sha256").update(require("node:fs").readFileSync(process.argv[1])).digest("hex"))' "$addon")
  local room="$RUNNER_TEMP/css-consumer-${CSS_EVIDENCE_ID}-${GITHUB_RUN_ATTEMPT}"
  check stage-consumer "$out/pack" "$CSS_CARRIER" "$room"
  if [[ "$CSS_CARRIER" == linux-* ]]; then
    local platform=linux/amd64
    if [[ "$CSS_CARRIER" == linux-arm64-gnu ]]; then platform=linux/arm64; fi
    if [[ "$CSS_CARRIER" == linux-arm64-gnu ]]; then
      cat /proc/sys/fs/binfmt_misc/qemu-aarch64 > "$out/qemu-registration.txt"
      local binfmt interpreter
      binfmt=$(docker image ls --format '{{.Repository}}:{{.Tag}}' | awk '/^tonistiigi\/binfmt:/ {print; count++} END {if (count != 1) exit 1}')
      docker image inspect --format '{{.Id}} {{json .RepoDigests}}' "$binfmt" > "$out/qemu-image.txt"
      interpreter=$(awk '$1 == "interpreter" {print $2}' "$out/qemu-registration.txt")
      run qemu-version docker run --rm --entrypoint "$interpreter" "$binfmt" --version
    fi
    run load-image docker pull --platform "$platform" node:24.0.0-bookworm-slim
    docker image inspect node:24.0.0-bookworm-slim > "$out/load-image.json"
    local image
    image=$(docker image inspect --format '{{.Id}}' node:24.0.0-bookworm-slim)
    run raw-native-load docker run --rm --platform "$platform" -v "$PWD:/w:ro" -w /w "$image" node scripts/smoke-css-node.mjs "packages/zfb-${CSS_CARRIER}/zfb-css.node"
    run packed-public-load docker run --rm --platform "$platform" -v "$room:/consumer" -w /consumer \
      -e "CSS_CARRIER=$CSS_CARRIER" -e "CSS_ADDON_SHA=$CSS_ADDON_SHA" -e "CSS_EXPECTED_RUNTIME=$CSS_EXPECTED_RUNTIME" -e "CSS_PACKAGE_VERSION=$CSS_PACKAGE_VERSION" \
      "$image" node consumer.mjs
    run cli-version docker run --rm --platform "$platform" -v "$PWD:/w:ro" -w /w "$image" "$binary" -V
    test "$(docker run --rm --platform "$platform" -v "$PWD:/w:ro" -w /w "$image" "$binary" -V)" = "zfb $CSS_PACKAGE_VERSION"
    if [[ "$CSS_CARRIER" == linux-arm64-gnu ]]; then
      echo 'execution=emulated-linux-arm64-QEMU' > "$out/execution.txt"
    else
      echo 'execution=native-linux-x64' > "$out/execution.txt"
    fi
  else
    "$ZFB_LOAD_NODE" -e 'if (process.version !== "v24.0.0" || `${process.platform}/${process.arch}` !== process.env.CSS_EXPECTED_RUNTIME) throw Error("native Node24.0.0 architecture mismatch")'
    run raw-native-load "$ZFB_LOAD_NODE" scripts/smoke-css-node.mjs "$addon"
    run packed-public-load "$ZFB_LOAD_NODE" "$room/consumer.mjs"
    run cli-version "$binary" -V
    test "$("$binary" -V)" = "zfb $CSS_PACKAGE_VERSION"
    echo 'execution=native' > "$out/execution.txt"
  fi
}
case "$mode" in
  native)
    run dependencies pnpm install --frozen-lockfile
    graph
    if [[ "$CSS_BUILDER" == cross ]]; then capture_cross before false || true; fi
    run cli-build "${CSS_BUILDER:?}" build -p zfb --release --target "$CSS_TARGET"
    run addon-build bash scripts/build-css-node.sh "$CSS_TARGET" "$CSS_BUILDER"
    if [[ "$CSS_BUILDER" == cross ]]; then capture_cross complete true; fi
    target_dir=$(cargo metadata --locked --no-deps --format-version 1 | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).target_directory))')
    if [[ "$RUNNER_OS" == Windows ]]; then target_dir=$(cygpath -u "$target_dir"); fi
    binary="$target_dir/$CSS_TARGET/release/zfb"
    library="$target_dir/$CSS_TARGET/release/$CSS_LIBRARY"
    if [[ "$RUNNER_OS" == Windows ]]; then binary="$binary.exe"; fi
    if [[ "$CSS_CARRIER" == linux-* ]]; then
      for file in "$binary" "$library"; do
        maximum=$(objdump -T "$file" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -V | tail -1)
        test -n "$maximum"
        test "$(printf '%s\n' "$maximum" GLIBC_2.34 | sort -V | tail -1)" = GLIBC_2.34
        printf '%s %s\n' "$file" "$maximum" >> "$out/glibc.txt"
      done
      docker image ls --digests --no-trunc > "$out/build-images.txt"
      cross --version >> "$out/toolchain.txt"
    fi
    cmp "$library" "packages/zfb-$CSS_CARRIER/zfb-css.node"
    cp "$binary" "packages/zfb-$CSS_CARRIER/$(basename "$binary")"
    chmod 0755 "packages/zfb-$CSS_CARRIER/$(basename "$binary")"
    pack_sdk
    pack_carrier "$PWD/packages/zfb-$CSS_CARRIER" "$binary" "$library"
    load_consumer "${binary#"$PWD/"}" "packages/zfb-$CSS_CARRIER/zfb-css.node"
    ;;
  fast-mac)
    test "$(uname -s):$(uname -m)" = Darwin:x86_64
    run producer bash scripts/build-macos-x64-local.sh --out-dir "$out/received"
    graph
    target_dir=$(cargo metadata --locked --no-deps --format-version 1 | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).target_directory))')
    library="$target_dir/x86_64-apple-darwin/release/libzfb_css_node.dylib"
    cmp "$library" packages/zfb-darwin-x64/zfb-css.node
    pack_sdk
    pack_carrier "$PWD/packages/zfb-darwin-x64" packages/zfb-darwin-x64/zfb "$library"
    load_consumer packages/zfb-darwin-x64/zfb packages/zfb-darwin-x64/zfb-css.node
    ;;
  ingestion)
    node --input-type=module - "$CSS_RECEIVED/identity.json" <<'JS'
import fs from 'node:fs';
const identity=JSON.parse(fs.readFileSync(process.argv[2]));
if (identity.sourceSHA !== process.env.SOURCE_SHA || identity.workflowSHA !== process.env.WORKFLOW_SHA || identity.target !== 'x86_64-apple-darwin') throw Error('received fast-Mac source/workflow/target tuple mismatch');
JS
    check ingest "$CSS_RECEIVED/received" packages/zfb-darwin-x64 "$out/ingested-carrier"
    pack_carrier "$out/ingested-carrier" "$out/ingested-carrier/zfb" "$out/ingested-carrier/zfb-css.node"
    run fault-cases "$py" "$tooling/test-css-carrier-verification.py" "$CSS_RECEIVED/received"
    ;;
  linux-size)
    run dependencies pnpm install --frozen-lockfile
    run size-toolchain "$py" "$tooling/css-carrier-verification.py" health-size-step 'Assert island-size toolchain'
    run native-build cargo build --locked --workspace --all-targets
    run size-pack "$py" "$tooling/css-carrier-verification.py" health-size-step 'Build and pack packages for island-size measurement'
    run size-measure "$py" "$tooling/css-carrier-verification.py" health-size-step 'Measure and enforce Linux x64 island-size budgets'
    cp -R target/island-size "$out/measurement"
    ;;
  *) echo 'Unsupported verification mode' >&2; exit 2 ;;
esac
check assert-source "$out/source-files.json"
printf '{"status":"passed","mode":"%s"}\n' "$mode" > "$out/success.json"
