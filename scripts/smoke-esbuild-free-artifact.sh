#!/usr/bin/env bash
set -euo pipefail

# scripts/smoke-esbuild-free-artifact.sh <tarball-dir>
#
# Installed-artifact proof that zfb neither ships nor runs esbuild (#4636,
# esbuild-removal epic #4628). Consumes the packed tarballs that
# scripts/smoke-packed-clean-room.sh exports with ZFB_SMOKE_TARBALL_OUT:
#
#   1. Inspects every packed zfb package (manifest dependency fields and
#      tarball entries) for an esbuild package or binary.
#   2. Installs create-zfb from its tarball, scaffolds a site and installs it
#      with every @takazudo/* package overridden to its tarball (the same
#      override scheme as the clean-room smoke; third-party dependencies come
#      from the registry), then walks `pnpm ls --json --depth Infinity`:
#      esbuild reached through a zfb package fails, esbuild reached only
#      through third-party packages is reported separately.
#   3. Runs the installed `zfb build` with esbuild unavailable — PATH entries
#      holding an `esbuild` removed, ZFB_ESBUILD_BIN and the retired bundler
#      selectors unset, a recording trap `esbuild` first on PATH — under
#      `strace -f -e trace=execve,connect` when strace is available. Any
#      esbuild exec (the trap included), curl/wget exec, non-loopback or
#      DNS connect fails the run; package-manager execs are recorded.
#   4. Checks the output works: the scaffold's expected content, every local
#      script/stylesheet the page references exists, and every emitted .js /
#      .mjs file parses as an ES module under Node.
#
# Env:
#   ZFB_SMOKE_PLATFORM               platform package under test (default linux-x64-gnu)
#   ZFB_ESBUILD_FREE_EVIDENCE_DIR    where reports and logs go (default: a temp dir)
#   ZFB_ESBUILD_FREE_REQUIRE_STRACE  1 = fail when strace is unavailable (CI sets it)
#
# The pure checks live in scripts/esbuild-free-artifact-audit.mjs and are unit
# tested offline by scripts/__tests__/esbuild-free-artifact-audit.test.mjs.

TARBALL_DIR="${1:-}"
ZFB_SMOKE_PLATFORM="${ZFB_SMOKE_PLATFORM:-linux-x64-gnu}"
PLATFORM_PKG="@takazudo/zfb-$ZFB_SMOKE_PLATFORM"
REQUIRE_STRACE="${ZFB_ESBUILD_FREE_REQUIRE_STRACE:-0}"

pass() { printf '[PASS] %s\n' "$1"; }
note() { printf '[NOTE] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

if [[ -z "$TARBALL_DIR" || ! -d "$TARBALL_DIR" ]]; then
  echo "usage: $0 <directory of packed .tgz tarballs>" >&2
  exit 2
fi
if [[ ! -f scripts/esbuild-free-artifact-audit.mjs ]]; then
  echo "::error::must be run from the repo root" >&2
  exit 2
fi
REPO_ROOT="$(pwd -P)"
AUDIT="$REPO_ROOT/scripts/esbuild-free-artifact-audit.mjs"
TARBALL_DIR="$(cd "$TARBALL_DIR" && pwd -P)"
EVIDENCE_DIR="${ZFB_ESBUILD_FREE_EVIDENCE_DIR:-$(mktemp -d)}"
mkdir -p "$EVIDENCE_DIR"
EVIDENCE_DIR="$(cd "$EVIDENCE_DIR" && pwd -P)"
echo "Evidence: $EVIDENCE_DIR"

# ── Stage 1: packed package contents ─────────────────────────────────────────
declare -A TARBALL_BY_NAME=()
shopt -s nullglob
for tarball in "$TARBALL_DIR"/*.tgz; do
  slug="$(basename "$tarball" .tgz)"
  tar -xzOf "$tarball" package/package.json >"$EVIDENCE_DIR/$slug.package.json" ||
    fail "$tarball has no package/package.json"
  tar -tzf "$tarball" >"$EVIDENCE_DIR/$slug.entries.txt"
  name="$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).name' "$EVIDENCE_DIR/$slug.package.json")"
  TARBALL_BY_NAME["$name"]="$tarball"
  if node "$AUDIT" manifest "$EVIDENCE_DIR/$slug.package.json" "$EVIDENCE_DIR/$slug.entries.txt" \
    >"$EVIDENCE_DIR/$slug.audit.json"; then
    pass "$name ships no esbuild package or binary ($(wc -l <"$EVIDENCE_DIR/$slug.entries.txt") entries)"
  else
    cat "$EVIDENCE_DIR/$slug.audit.json" >&2
    fail "$name ships or declares esbuild"
  fi
done
shopt -u nullglob
for required in @takazudo/zfb @takazudo/zfb-runtime @takazudo/zfb-slugify create-zfb "$PLATFORM_PKG"; do
  [[ -n "${TARBALL_BY_NAME[$required]:-}" ]] || fail "no packed tarball for $required in $TARBALL_DIR"
done

# ── Stage 2: install, scaffold, dependency closure ───────────────────────────
write_overrides() {
  # $1 = install root; overrides every packed @takazudo/* package.
  local root="$1" name
  {
    echo "minimumReleaseAge: 0"
    echo "overrides:"
    for name in "${!TARBALL_BY_NAME[@]}"; do
      if [[ "$name" == @takazudo/* ]]; then
        printf '  "%s": "file:%s"\n' "$name" "${TARBALL_BY_NAME[$name]}"
      fi
    done
  } >"$root/pnpm-workspace.yaml"
}

CLEAN_ROOM="$(mktemp -d)"
cat >"$CLEAN_ROOM/package.json" <<JSON
{
  "name": "zfb-esbuild-free-clean-room",
  "private": true,
  "version": "0.0.0",
  "dependencies": { "create-zfb": "file:${TARBALL_BY_NAME[create-zfb]}" }
}
JSON
write_overrides "$CLEAN_ROOM"
(cd "$CLEAN_ROOM" && pnpm install) >"$EVIDENCE_DIR/clean-room-install.log" 2>&1 ||
  { tail -40 "$EVIDENCE_DIR/clean-room-install.log" >&2; fail "clean-room install of create-zfb failed"; }

SITE_NAME="esbuild-free-site"
SITE_DIR="$CLEAN_ROOM/$SITE_NAME"
(cd "$CLEAN_ROOM" && node node_modules/create-zfb/bin/create-zfb.mjs "$SITE_NAME") \
  >"$EVIDENCE_DIR/scaffold.log" 2>&1 ||
  { tail -40 "$EVIDENCE_DIR/scaffold.log" >&2; fail "scaffold via packed create-zfb failed"; }
[[ -f "$SITE_DIR/package.json" ]] || fail "scaffold did not produce $SITE_DIR/package.json"
# `zfb new`'s own best-effort install targets the registry; start clean.
rm -rf "$SITE_DIR/node_modules" "$SITE_DIR/pnpm-lock.yaml"
write_overrides "$SITE_DIR"
(cd "$SITE_DIR" && pnpm install) >"$EVIDENCE_DIR/site-install.log" 2>&1 ||
  { tail -40 "$EVIDENCE_DIR/site-install.log" >&2; fail "site install from packed tarballs failed"; }
pass "scaffolded $SITE_NAME and installed it from the packed tarballs"

(cd "$SITE_DIR" && pnpm ls --json --depth Infinity) >"$EVIDENCE_DIR/pnpm-ls.json"
if node "$AUDIT" closure "$EVIDENCE_DIR/pnpm-ls.json" >"$EVIDENCE_DIR/closure.json"; then
  pass "no esbuild package is reachable through a zfb package"
else
  cat "$EVIDENCE_DIR/closure.json" >&2
  fail "esbuild is reachable through a zfb package (see closure.json firstParty)"
fi
THIRD_PARTY="$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).thirdParty.join("\n")' "$EVIDENCE_DIR/closure.json")"
if [[ -n "$THIRD_PARTY" ]]; then
  note "third-party packages bring esbuild transitively (not shipped by zfb):"
  printf '%s\n' "$THIRD_PARTY" | sed 's/^/  /'
else
  note "no third-party package brings esbuild either"
fi
INSTALLED_ESBUILD="$(find -L "$SITE_DIR/node_modules" -maxdepth 6 -type f -path '*/bin/esbuild' 2>/dev/null | head -5 || true)"
[[ -n "$INSTALLED_ESBUILD" ]] && note "esbuild binaries present in the install (third-party): $INSTALLED_ESBUILD"

# ── Stage 3: traced build with esbuild unavailable ───────────────────────────
TRAP_DIR="$(mktemp -d)"
TRAP_LOG="$EVIDENCE_DIR/esbuild-trap.log"
cat >"$TRAP_DIR/esbuild" <<SH
#!/bin/sh
printf 'esbuild invoked: %s\n' "\$*" >>'$TRAP_LOG'
exit 97
SH
chmod 0755 "$TRAP_DIR/esbuild"
BUILD_PATH="$TRAP_DIR"
IFS=':' read -r -a PATH_DIRS <<<"$PATH"
for dir in "${PATH_DIRS[@]}"; do
  [[ -z "$dir" ]] && continue
  if [[ -e "$dir/esbuild" ]]; then
    note "dropped $dir from PATH (holds esbuild)"
    [[ -x "$dir/node" && ! -e "$TRAP_DIR/node" ]] && ln -s "$dir/node" "$TRAP_DIR/node"
    continue
  fi
  BUILD_PATH="$BUILD_PATH:$dir"
done

ZFB_CLI="$SITE_DIR/node_modules/.bin/zfb"
[[ -x "$ZFB_CLI" ]] || fail "the installed site has no node_modules/.bin/zfb"
BUILD_ENV=(env -u ZFB_ESBUILD_BIN -u ZFB_BUNDLER -u ZFB_ROLLDOWN_PROTOTYPE "PATH=$BUILD_PATH")
(cd "$SITE_DIR" && "${BUILD_ENV[@]}" "$ZFB_CLI" --version) >"$EVIDENCE_DIR/zfb-version.txt"
grep -q "embedded bundler: rolldown-" "$EVIDENCE_DIR/zfb-version.txt" ||
  fail "installed zfb --version does not name the embedded Rolldown bundler: $(cat "$EVIDENCE_DIR/zfb-version.txt")"
pass "installed zfb reports $(grep 'embedded bundler' "$EVIDENCE_DIR/zfb-version.txt")"

rm -rf "$SITE_DIR/dist"
STRACE_LOG="$EVIDENCE_DIR/build.strace"
if command -v strace >/dev/null 2>&1; then
  STRACE_BIN="$(command -v strace)"
  if ! (cd "$SITE_DIR" && "${BUILD_ENV[@]}" "$STRACE_BIN" -f -qq -s 256 -e trace=execve,connect \
    -o "$STRACE_LOG" "$ZFB_CLI" build) >"$EVIDENCE_DIR/zfb-build.log" 2>&1; then
    tail -60 "$EVIDENCE_DIR/zfb-build.log" >&2
    fail "traced esbuild-free zfb build failed"
  fi
  if node "$AUDIT" strace "$STRACE_LOG" >"$EVIDENCE_DIR/strace-audit.json"; then
    pass "traced build: no esbuild exec, no downloader exec, no network connect ($(node -p 'require(process.argv[1]).execCount' "$EVIDENCE_DIR/strace-audit.json") execs)"
  else
    cat "$EVIDENCE_DIR/strace-audit.json" >&2
    fail "the traced build exec'd esbuild or a downloader, or opened a network connection"
  fi
else
  [[ "$REQUIRE_STRACE" == "1" ]] && fail "strace is required (ZFB_ESBUILD_FREE_REQUIRE_STRACE=1) but not installed"
  echo "::warning::strace unavailable: exec/connect tracing NOT performed; only the PATH trap guards this build" >&2
  if ! (cd "$SITE_DIR" && "${BUILD_ENV[@]}" "$ZFB_CLI" build) >"$EVIDENCE_DIR/zfb-build.log" 2>&1; then
    tail -60 "$EVIDENCE_DIR/zfb-build.log" >&2
    fail "esbuild-free zfb build failed"
  fi
fi
[[ -e "$TRAP_LOG" ]] && fail "the build invoked the trap esbuild: $(cat "$TRAP_LOG")"
pass "the trap esbuild on PATH was never invoked"

# ── Stage 4: the output works ────────────────────────────────────────────────
DIST="$SITE_DIR/dist"
[[ -f "$DIST/index.html" ]] || fail "dist/index.html not found after the esbuild-free build"
grep -q "basic-blog" "$DIST/index.html" || fail "dist/index.html lacks the scaffold heading (basic-blog)"
grep -q "Hello, zfb" "$DIST/index.html" || fail "dist/index.html does not list the seed post (Hello, zfb)"
pass "dist/index.html renders the scaffold content"

REFERENCED=0
while IFS= read -r ref; do
  [[ -f "$DIST/${ref#/}" ]] || fail "dist/index.html references $ref, which was not emitted"
  REFERENCED=$((REFERENCED + 1))
done < <(grep -oE '(src|href)="/[^"?#]+\.(js|mjs|css)"' "$DIST/index.html" | sed -E 's/^(src|href)="//; s/"$//' | sort -u)
[[ "$REFERENCED" -gt 0 ]] || fail "dist/index.html references no local script or stylesheet"
pass "every local script/stylesheet referenced by index.html exists ($REFERENCED)"

PARSED=0
while IFS= read -r -d '' module; do
  node --input-type=module --check <"$module" 2>"$EVIDENCE_DIR/module-check.err" ||
    { cat "$EVIDENCE_DIR/module-check.err" >&2; fail "emitted ${module#"$DIST"/} does not parse as an ES module"; }
  PARSED=$((PARSED + 1))
done < <(find "$DIST" -type f \( -name '*.js' -o -name '*.mjs' \) -print0)
[[ "$PARSED" -gt 0 ]] || fail "the build emitted no JavaScript to check"
pass "all $PARSED emitted JavaScript files parse as ES modules"

echo "All esbuild-free installed-artifact assertions passed."
