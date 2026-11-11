#!/usr/bin/env bash
# build-darwin-local.sh — owner-Mac producer for BOTH Darwin release inputs (#4689).
#
# Builds aarch64-apple-darwin (native) and x86_64-apple-darwin (qualified under
# Rosetta 2) from the EXACT release source, writes the locked release assets and
# one qualification receipt per target, self-admits them with
# scripts/admit-darwin-artifacts.mjs, and optionally uploads them to a DRAFT
# GitHub Release without ever overwriting an asset. No hosted macOS runner is
# involved. Contract: scripts/darwin-artifact-contract.md.
#
# Usage:
#   scripts/build-darwin-local.sh --source <sha> [--tag v<version>] [--out-dir DIR]
#                                 [--upload v<version>] [--check-only]
#
#   --source SHA   required; must equal HEAD (full object id)
#   --tag TAG      optional; must be v<packages/zfb version> and peel to SHA
#   --out-dir DIR  assets + receipts; must lie outside the checkout
#                  (default: ${TMPDIR:-/tmp}/zfb-darwin-<version>-<sha12>)
#   --upload TAG   upload to the existing DRAFT Release TAG (implies --tag TAG)
#   --check-only   run the argument/source-binding refusals and stop (no build)
#
# Environment:
#   ZFB_X64_NODE     x64 Node >=24 executable for the Rosetta addon smoke (required)
#   CARGO_BUILD_JOBS honored by cargo and recorded in the receipt (set 1 on a low-memory Mac)
#   HEAVY_GUARD      heavy-guard executable; default $HOME/.claude/scripts/heavy-guard.sh,
#                    then $HOME/.codex/scripts/heavy-guard.sh, else run directly
#   RUSTUP_TOOLCHAIN process-local toolchain pin (never rustup default/override)
set -euo pipefail

TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

die() {
  echo "ERROR: $*" >&2
  exit 1
}

source_sha=""
tag=""
out_dir=""
upload_tag=""
check_only=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --source | --tag | --out-dir | --upload)
      [[ $# -ge 2 && -n "$2" ]] || die "$1 needs a value"
      case "$1" in
        --source) source_sha="$2" ;;
        --tag) tag="$2" ;;
        --out-dir) out_dir="$2" ;;
        --upload) upload_tag="$2" ;;
      esac
      shift 2
      ;;
    --check-only)
      check_only=1
      shift
      ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ -n "$source_sha" ]] || die "--source <sha> is required"
[[ "$source_sha" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || die "--source must be a full lowercase hex commit id"
if [[ -n "$upload_tag" ]]; then
  [[ -z "$tag" || "$tag" == "$upload_tag" ]] || die "--upload $upload_tag conflicts with --tag $tag"
  tag="$upload_tag"
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

# ── Source binding: clean tree at exactly --source (and the tag's commit) ────

if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  git status --short >&2
  die "working tree is dirty (tracked changes or untracked non-ignored files)"
fi
head="$(git rev-parse --verify HEAD)"
[[ "$head" == "$source_sha" ]] || die "--source $source_sha != HEAD $head"
version="$(node -p "require('./packages/zfb/package.json').version")"
if [[ -n "$tag" ]]; then
  [[ "$tag" == "v$version" ]] || die "--tag $tag != v$version (packages/zfb/package.json)"
  tag_commit="$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}")" || die "tag $tag not found locally (git fetch --tags)"
  [[ "$tag_commit" == "$source_sha" ]] || die "tag $tag peels to $tag_commit, not $source_sha"
fi
tree="$(git rev-parse --verify "$source_sha^{tree}")"

if [[ -z "$out_dir" ]]; then
  out_dir="${TMPDIR:-/tmp}/zfb-darwin-${version}-${source_sha:0:12}"
fi
mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd -P)"
case "$out_dir/" in
  "$repo_root"/*) die "--out-dir $out_dir is inside the source checkout" ;;
esac

echo "==> source $source_sha (tree $tree), version $version${tag:+, tag $tag}"
echo "==> out dir $out_dir"
if [[ "$check_only" -eq 1 ]]; then
  echo "==> --check-only: source binding OK; nothing built"
  exit 0
fi

# ── Host prerequisites ──────────────────────────────────────────────────────

[[ "$(uname -s)" == "Darwin" ]] || die "must run on macOS; a non-Mac host can only consume uploaded, qualified inputs"
[[ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" == "1" ]] || die "requires an Apple Silicon Mac (arm64 native + Rosetta 2)"
arch -x86_64 /usr/bin/true 2>/dev/null || die "Rosetta 2 is not installed (softwareupdate --install-rosetta)"
x64_node="${ZFB_X64_NODE:-}"
[[ -n "$x64_node" && -x "$x64_node" ]] || die "set ZFB_X64_NODE to an x64 Node >=24 executable"
[[ -z "$(find "$out_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]] || die "--out-dir $out_dir is not empty"
command -v rustup >/dev/null 2>&1 || die "rustup not found"
if cargo --version 2>/dev/null | grep -q '(Homebrew)'; then
  # Process-local PATH fix only: Homebrew's host-only rust cannot build x86_64.
  toolchain_bin="$HOME/.rustup/toolchains/$(rustup show active-toolchain | awk '{print $1}')/bin"
  [[ -d "$toolchain_bin" ]] || die "rustup toolchain bin not found: $toolchain_bin"
  export PATH="$toolchain_bin:$PATH"
fi

heavy() {
  local guard="${HEAVY_GUARD:-}" candidate
  if [[ -z "$guard" ]]; then
    for candidate in "$HOME/.claude/scripts/heavy-guard.sh" "$HOME/.codex/scripts/heavy-guard.sh"; do
      if [[ -x "$candidate" ]]; then
        guard="$candidate"
        break
      fi
    done
  fi
  if [[ -n "$guard" ]]; then
    "$guard" -- "$@"
  else
    "$@"
  fi
}

for target in "${TARGETS[@]}"; do
  rustup target add "$target"
done
rustc_vv="$(rustc -vV)"
cargo_v="$(cargo -V)"

echo "==> pnpm install --frozen-lockfile (build.rs embeds node_modules)"
CI=true pnpm install --frozen-lockfile

target_root="$(cargo metadata --format-version 1 --no-deps |
  node -e "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>console.log(JSON.parse(d).target_directory))")"
work="$(mktemp -d "${TMPDIR:-/tmp}/zfb-darwin-work.XXXXXX")"
trap 'rm -rf "$work"' EXIT
contract=(node "$repo_root/scripts/darwin-artifact-contract.mjs")

# ── Build (serial: one target, one command at a time) ───────────────────────

for target in "${TARGETS[@]}"; do
  case "$target" in
    aarch64-apple-darwin) carrier=darwin-arm64 ;;
    x86_64-apple-darwin) carrier=darwin-x64 ;;
  esac
  echo "==> cargo build -p zfb --release --target $target"
  ZFB_RELEASE_VERSION="$version" heavy cargo build -p zfb --release --target "$target"
  echo "==> CSS addon for $target"
  heavy bash scripts/build-css-node.sh "$target" cargo

  archive="zfb-${version}-${target}.tar.gz"
  addon="zfb-css-${version}-${target}.node"
  stage="$work/stage-$target"
  mkdir -p "$stage"
  cp "$target_root/$target/release/zfb" "$stage/zfb"
  chmod 0755 "$stage/zfb"
  # COPYFILE_DISABLE + --no-mac-metadata/--no-xattrs stop bsdtar from adding a
  # "._zfb" AppleDouble entry, which would break the one-root-file contract.
  COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C "$stage" -czf "$out_dir/$archive" zfb
  cp "packages/zfb-$carrier/zfb-css.node" "$out_dir/$addon"
  chmod 0644 "$out_dir/$addon"
  (cd "$out_dir" && shasum -a 256 "$archive" >"$archive.sha256" && shasum -a 256 "$addon" >"$addon.sha256")
done

if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  git status --short >&2
  die "the build modified the source checkout; receipts would not describe $source_sha"
fi

# ── Qualification (each check runs once; no retries) ───────────────────────

run_check() {
  local checks="$1" name="$2" expect="$3"
  shift 3
  local out="$work/check-$name.out" code=0
  "$@" >"$out" 2>&1 || code=$?
  local expect_args=()
  [[ -n "$expect" ]] && expect_args=(--expect "$expect")
  "${contract[@]}" record-check --file "$checks" --name "$name" --exit-code "$code" \
    --output-file "$out" ${expect_args[@]+"${expect_args[@]}"} -- "$@"
}

hw_arm64="$(sysctl -n hw.optional.arm64)"
os_version="$(sw_vers -productName) $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
status=0
for target in "${TARGETS[@]}"; do
  archive="zfb-${version}-${target}.tar.gz"
  addon="$out_dir/zfb-css-${version}-${target}.node"
  qual="$work/qual-$target"
  mkdir -p "$qual"
  tar -C "$qual" -xzf "$out_dir/$archive"
  bin="$qual/zfb"
  checks="$work/checks-$target.jsonl"
  : >"$checks"
  if [[ "$target" == "aarch64-apple-darwin" ]]; then
    run=(arch -arm64)
    node_bin="$(command -v node)"
    lipo_arch=arm64
    node_arch=arm64
    translated_expect=0
  else
    run=(arch -x86_64)
    node_bin="$x64_node"
    lipo_arch=x86_64
    node_arch=x64
    translated_expect=1
  fi
  run_check "$checks" macho-binary-arch "$lipo_arch" lipo -archs "$bin"
  run_check "$checks" macho-addon-arch "$lipo_arch" lipo -archs "$addon"
  # sysctl itself is the measured process: under arch -x86_64 it runs translated.
  run_check "$checks" proc-translated "$translated_expect" "${run[@]}" /usr/sbin/sysctl -n sysctl.proc_translated
  run_check "$checks" cli-version "zfb $version" "${run[@]}" "$bin" -V
  run_check "$checks" cli-help "" "${run[@]}" "$bin" --help
  run_check "$checks" node-identity "$node_arch" "${run[@]}" "$node_bin" -e \
    'if (Number(process.versions.node.split(".")[0]) < 24) process.exit(3); console.log(process.arch)'
  run_check "$checks" css-addon-smoke "" "${run[@]}" "$node_bin" scripts/smoke-css-node.mjs "$addon"

  translated="$("${run[@]}" /usr/sbin/sysctl -n sysctl.proc_translated)"
  uname_m="$("${run[@]}" /usr/bin/uname -m)"
  "${contract[@]}" write-receipt --target "$target" --version "$version" ${tag:+--tag "$tag"} \
    --commit "$source_sha" --tree "$tree" --repo-root "$repo_root" --out-dir "$out_dir" \
    --binary "$bin" --checks "$checks" --uname-m "$uname_m" --os-version "$os_version" \
    --hw-arm64 "$hw_arm64" --translated "$translated" --rustc-vv "$rustc_vv" --cargo-v "$cargo_v" \
    --file-binary "$(file -b "$bin")" --file-addon "$(file -b "$addon")" \
    ${CARGO_BUILD_JOBS:+--cargo-build-jobs "$CARGO_BUILD_JOBS"} || status=1
done
[[ "$status" -eq 0 ]] || die "qualification failed; receipts with status=failed are in $out_dir (do not upload)"

# ── Self-admission: the same verifier release.yml runs on Linux ─────────────

node scripts/admit-darwin-artifacts.mjs --dir "$out_dir" --version "$version" \
  --source-commit "$source_sha" --source-tree "$tree" --repo-root "$repo_root" \
  --json-out "$work/admission.json" >/dev/null || die "self-admission rejected the outputs (see REJECT lines)"
echo "==> admitted locally; assets in $out_dir"
ls -l "$out_dir"

# ── Optional upload: DRAFT Release only, never overwrite ─────────────────────

[[ -n "$upload_tag" ]] || exit 0
is_draft="$(gh release view "$upload_tag" --json isDraft --jq '.isDraft')" || die "no GitHub Release for $upload_tag"
[[ "$is_draft" == "true" ]] || die "Release $upload_tag is not a draft; refusing to upload"
existing="$(gh release view "$upload_tag" --json assets --jq '.assets[].name')"
to_upload=()
for target in "${TARGETS[@]}"; do
  while IFS= read -r name; do
    if grep -Fxq "$name" <<<"$existing"; then
      remote="$work/remote"
      rm -rf "$remote" && mkdir -p "$remote"
      gh release download "$upload_tag" --pattern "$name" --dir "$remote"
      if [[ "$(shasum -a 256 <"$remote/$name" | awk '{print $1}')" == "$(shasum -a 256 <"$out_dir/$name" | awk '{print $1}')" ]]; then
        echo "    $name already uploaded with identical bytes — skipping"
      else
        die "$name already exists on $upload_tag with DIFFERENT bytes; refusing to overwrite (delete it deliberately first)"
      fi
    else
      to_upload+=("$out_dir/$name")
    fi
  done < <("${contract[@]}" asset-names --version "$version" --target "$target")
done
if [[ "${#to_upload[@]}" -gt 0 ]]; then
  gh release upload "$upload_tag" "${to_upload[@]}"
fi
echo "==> $upload_tag holds every Darwin input; release.yml must still admit them before publishing"
