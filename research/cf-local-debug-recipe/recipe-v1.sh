#!/usr/bin/env bash
# recipe-v1.sh — local Cloudflare Workers dev loop for zfb projects
#
# Usage (from the project root):
#   bash /path/to/recipe-v1.sh [--port PORT] [--db DB_NAME]
#
# Requires:
#   - wrangler in PATH (or as a devDependency)
#   - pnpm build script that runs "zfb build" (plus any post-build steps)
#   - npx chokidar-cli@3 and npx concurrently@9 (fetched on demand via npx cache)
#
# Handles:
#   - Initial D1 local migration apply (idempotent, safe to re-run)
#   - Port collision detection (exits early with a clear message)
#   - Concurrent pnpm-build watcher + wrangler pages dev in one process tree
#   - Graceful Ctrl-C via concurrently --kill-others

set -euo pipefail

PORT="${1:-8788}"
DB_NAME="${2:-}"

# Allow flag-style overrides: --port 8889 --db mydb
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --db)   DB_NAME="$2"; shift 2 ;;
    *) shift ;;
  esac
done

# ── Preflight checks ──────────────────────────────────────────────────────────

# Confirm we're in a zfb project with wrangler.toml
if [[ ! -f wrangler.toml ]]; then
  echo "error: wrangler.toml not found. Run this script from the project root." >&2
  exit 1
fi

if [[ ! -f package.json ]]; then
  echo "error: package.json not found. Run this script from the project root." >&2
  exit 1
fi

# Check for pnpm
if ! command -v pnpm &>/dev/null; then
  echo "error: pnpm not found in PATH." >&2
  exit 1
fi

# Check for wrangler
if ! command -v wrangler &>/dev/null && ! pnpm exec wrangler --version &>/dev/null 2>&1; then
  echo "error: wrangler not found. Install with: npm i -g wrangler  or  pnpm add -D wrangler" >&2
  exit 1
fi

# Port collision check
if lsof -ti TCP:"$PORT" &>/dev/null 2>&1; then
  echo "error: port $PORT is already in use." >&2
  echo "  Kill the existing process:  lsof -ti TCP:$PORT | xargs kill" >&2
  echo "  Or use a different port:    $0 --port 8789" >&2
  exit 1
fi

# ── D1 Migrations ─────────────────────────────────────────────────────────────

# Auto-detect DB name from wrangler.toml if not provided
if [[ -z "$DB_NAME" ]]; then
  DB_NAME=$(grep -m1 'database_name' wrangler.toml | sed 's/.*= *"\([^"]*\)".*/\1/' || true)
fi

if [[ -n "$DB_NAME" ]]; then
  echo "Applying local D1 migrations for database: $DB_NAME"
  wrangler d1 migrations apply "$DB_NAME" --local 2>&1 | tail -5 || {
    echo "warning: D1 migration failed. Continuing anyway — DB may already be up to date." >&2
  }
else
  echo "note: No D1 database name found in wrangler.toml — skipping migration step."
fi

# ── Initial build ─────────────────────────────────────────────────────────────

echo "Running initial build..."
pnpm build 2>&1

echo ""
echo "Starting dev loop on http://localhost:$PORT"
echo "  Edit any file in pages/ components/ lib/ styles/ and the page will refresh within ~2s."
echo "  Press Ctrl-C to stop."
echo ""

# ── Concurrent watcher + wrangler ─────────────────────────────────────────────
#
# Two concurrent processes:
#   wrangler  — watches dist/ for changes and reloads the worker automatically
#   chokidar  — watches source directories; on any change, runs pnpm build
#
# concurrently --kill-others ensures both die together on Ctrl-C.
#
# NOTE on wrangler reload semantics (observed 2026-05-27, wrangler 4.72.0):
#   wrangler pages dev watches dist/_zfb_inner.mjs (and other dist/ files) for
#   content changes. Each successful pnpm build produces a new _zfb_inner.mjs
#   with updated content, which triggers an automatic "Reloading local server..."
#   within ~300ms. No manual restart is needed.
#
# NOTE on chokidar watch paths:
#   Adjust the glob patterns below if your project uses different source dirs.
#   The pattern below covers the standard zfb-example-webshop layout.

npx --yes concurrently@9 \
  --names "wrangler,watch" \
  --prefix-colors "cyan,yellow" \
  --kill-others \
  --kill-others-on-fail \
  "wrangler pages dev dist/ --port $PORT" \
  "npx --yes chokidar-cli@3 'pages/**/*.tsx' 'components/**/*.tsx' 'layouts/**/*.tsx' 'lib/**/*.ts' 'styles/**/*.css' --command 'pnpm build' --initial false --debounce 200"
