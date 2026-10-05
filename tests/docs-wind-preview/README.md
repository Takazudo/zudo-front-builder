# Built docs wind preview browser check

This Chromium suite exercises every built utility family in English and
Japanese, all ten paired guide pages, and the installed zudo-doc `HtmlPreview`
controls. It verifies all positive record/asset/source pairs, that expected
diagnostics stay textual, and representative computed layout, spacing,
typography, image, fragment, transition, reset, cascade, and variant behavior.
The Gap/Padding pilots also cover copy, keyboard and viewport controls, theme
contrast, old hashes, SPA remounts, and fresh 1440px/390px screenshots.

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
The utility and guide routes run at the built tree's actual root or configured
prefix. The job separately checks all 116 captured historical wind routes, 42
adjacent routes, their emitted local links/fragments, and inbound fragments
from other built docs pages.

Failure traces retain actions, screenshots and source files, with DOM snapshots
disabled. HtmlPreview uses sandboxed `about:srcdoc` documents without script
permission; Playwright's DOM snapshotter injects an init script into every frame,
which produces a blocked-script console warning there. The suite still fails on
console errors, page errors, failed requests and local HTTP errors, but traces do
not include HAR-style network events.
