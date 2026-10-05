# Built docs wind preview browser check

This focused Chromium suite exercises the built static docs site and the
installed zudo-doc `HtmlPreview` controls. It reads the shared Gap/Padding
records and generated public assets to check source, copied code, reset
isolation, computed values, locale controls, theme behavior and route remounts.

Build the docs tree first, then run from the repository root:

```sh
pnpm exec playwright test --config tests/docs-wind-preview/playwright.config.mjs
```

For a genuine subpath build, use the checked-in helper to temporarily set the
literal `base` config, build, and restore the config. Pass that same prefix as
`WIND_DOCS_BASE` only to the browser runner/server, which mounts the untouched
`dist/` under that path:

```sh
node docs/scripts/build-wind-docs-base.mjs /wind-docs-preview/
WIND_DOCS_BASE=/wind-docs-preview/ pnpm exec playwright test --config tests/docs-wind-preview/playwright.config.mjs
```

The test server maps requests under the configured prefix onto the built tree
and rejects requests outside it. It does not rewrite emitted HTML or URLs.
`WIND_DOCS_DIST` can point the runner at another built output directory.
Screenshots are captured from fresh contexts at 1440×1000 and 390×844 under
`$TMPDIR/zfb-wind-doc-preview-screenshots` (override with
`WIND_DOCS_SCREENSHOT_DIR`). Set `WIND_DOCS_CAPTURE_SCREENSHOTS=0` to skip them.

Failure traces retain actions, screenshots and source files, with DOM snapshots
disabled. HtmlPreview uses sandboxed `about:srcdoc` documents without script
permission; Playwright's DOM snapshotter injects an init script into every frame,
which produces a blocked-script console warning there. The suite still fails on
console errors, page errors, failed requests and local HTTP errors, but traces do
not include HAR-style network events.
