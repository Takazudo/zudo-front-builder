# Design workshop integration checks

The native EN/JA tool uses the existing ZFB 2.20.2 / zudo-doc 5.27 / Preact host.
`model.js` is the pure seed/value/export boundary; `workshop.js` owns one mounted
view and disposes every listener, timer, dialog and download URL. Only a validated,
versioned snapshot survives in host window memory between native route mounts.

The generator also writes editable Everyday assets into both bundled starters:

```sh
node scripts/generate-starter-designs.mjs --check
pnpm exec vp test run scripts/__tests__/starter-designs.test.mjs scripts/__tests__/verify-design-seed-exports.test.mjs
node --test docs/scripts/__tests__/design-workshop-model.test.mjs
```

Build the docs and run their type/content, HTML and island guards before browser
checks. Run heavy commands through the machine's required heavy/browser guards.

```sh
pnpm install --frozen-lockfile
pnpm docs:check
pnpm --filter docs test
pnpm docs:build
pnpm --filter docs check:html
pnpm --filter docs check:islands
pnpm exec playwright test --config tests/design-system-workshop/playwright.config.mjs
```

`ZFB_CHROMIUM_PATH` optionally selects an installed Chromium executable. The
browser suite proves native EN/JA hydration and navigation restoration, disposal,
keyboard role selection, modes/reset, theme separation, wide/mobile layout and
exact current clipboard/download bytes for all three edited seeds. Screenshots
wait for toast dismissal and disable animations to capture settled host width.

The starter browser test additionally requires `ZFB_BASIC_BLOG_DIST` and
`ZFB_NODE_FREE_DIST`, pointing to real scaffolded builds from a freshly rebuilt
binary. Without these paths it explicitly skips; a docs-only run is not starter
verification. It checks actual tokens, responsive layout, static Node-free
output, blog theme persistence and retained routes.

Templates are embedded at compile time. Build the current clean revision, run
`tests/smoke/node-free/run.sh`, and run `tests/smoke/node-free-ts/run.sh` with Node
absent from PATH. The TS fixture preserves the starter's explicit tokens while
adding its discriminator collection. For Basic blog use
`scripts/smoke-packed-clean-room.sh` with `ZFB_LINUX_X64_BINARY` pointing to that
binary and optional `ZFB_SMOKE_DIST_OUT` for browser input. The packed smoke uses
local tarballs and does not publish packages. Use normal packaging umask 022;
a session umask 077 changes installed executable modes and fails its 0755 check.

Run actual Wind export verification from a clean revision:

```sh
ZFB_BINARY="$PWD/target/debug/zfb" \
ZFB_BINARY_SOURCE_SHA="$(git rev-parse HEAD)" \
ZFB_BINARY_PROVENANCE="cargo build -p zfb --locked at this clean revision" \
node scripts/verify-design-seed-exports.mjs
```

This builds six projects (original and all-controls-edited for each seed), checks
five-file archive equality/CRC, actual emitted CSS/HTML, 31 utility candidates,
Wind audit/explain and current variable values. The evidence directory records
source SHA, binary hash, per-file hashes and logs. The browser specimen is
explicitly authored CSS using the same generated variables, not a claim of
browser-side Rust compilation.

The committed screenshots show the implemented native host and fresh bundled
starters. They are review evidence, not pixel-baseline assertions. No docs-host
migration, initializer seed flag, release, package publication or deployment
workflow change is included. Issue #3990 remains open.
