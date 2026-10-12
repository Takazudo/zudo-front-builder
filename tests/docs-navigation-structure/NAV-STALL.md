# EN compatibility navigation stall profiler (#4166)

`nav-stall-profile.mjs` reproduces and attributes the two observations in [#4156](https://github.com/Takazudo/zudo-front-builder/issues/4156). It is diagnostics, not a gate: it exits 0 whenever the run completes, including rounds that stall, fail to swap, or never settle. It exits 1 only when the run itself fails, for example when a route does not answer HTTP 200.

1. **Layerize after swap.** On the EN Wind compatibility page at 1440×1000, 1–2 Layerize long tasks of about 4–7 s follow a soft navigation. JA and 390 px do not show them.
2. **Wrangler-only gap.** On Wrangler, 6.1 s passed between `pushState` and `zfb:before-swap`. A plain static server showed no gap in 21 runs ([evidence comment](https://github.com/Takazudo/zudo-front-builder/issues/4156#issuecomment-6120198837)).

Nothing below is a finding until a retained run supports it. The candidate lists are hypotheses, each paired with the experiment that would confirm or refute it.

## What one run records

- **Identity:** source SHA, branch and dirty flag; installed `@takazudo/zfb`, `zfb-runtime`, `zudo-doc`, `zfb-adapter-cloudflare` and `wrangler` versions read from `docs/node_modules`; `zfb --version`; Chromium version; OS, CPU and memory; headless mode; viewport and DPR; reduced-motion; view-transition mode; extra CSS.
- **Tree identity:** a preflight request to every route on every host records the served HTML sha256, the matching `docs/dist/**/index.html` sha256, and whether they are equal (`servedMatchesDist`). This shows both hosts served one built tree.
- **Assets:** every script, stylesheet, document and fetch response is recorded by URL, with status, `content-encoding`, `cache-control`, sha256 and byte count.
- **Transitions:** per host and case, the `zfb-view-transitions-enabled` meta, the fallback meta, native `startViewTransition` support, and the effective `prefers-reduced-motion`.
- **Per navigation:** the times of click, router `fetch` start/headers, resource-timing `responseStart`/`responseEnd`, `pushState`, `startViewTransition` and its update callback, `ready`/`updateCallbackDone`/`finished`, and every `zfb:*` event the installed runtime dispatches (`before-preparation`, `after-preparation`, `before-swap`, `after-swap`, `page-load`, `navigation-aborted`). All times are relative to the click. Each navigation also records `longtask` entries, Long Animation Frames with top scripts, rAF frame gaps, URL correctness, and the settle state before and after.
- **Trace:** one CDP trace per measured navigation, written as `<host>-<case>-<round>-<enter|leave>.json.gz` under the trace directory. It records renderer-main-thread totals for Layerize, Layout, UpdateLayoutTree, PrePaint, Paint, UpdateLayerTree, Commit, script and GC. Each is split by phase (`after:click`, `after:pushState`, `after:vt-start`, `after:zfb:before-swap`, `after:zfb:after-swap`, `after:zfb:page-load`, `after:vt-finished`). The ten longest `RunTask`s are listed with wall time, CPU time (`cpuMs`) and their dominant children. The 15 event names with the largest totals are listed too, so a cost under an unexpected name is still visible.
- **Census:** once per host, case and route, in the first measured round and outside the trace window. It counts nodes, tables, rows, `code` and links, plus elements that are sticky, fixed, `will-change`, transformed, `backdrop-filter`, filtered, translucent, z-indexed, `contain` or `content-visibility`. It also counts scroll containers and overflowing scroll containers, overflowing tables per wrapper, view-transition names, document and `main` scroll size, and desktop sidebar display and size. `NAV_STALL_LAYER_TREE=1` adds a CDP composited-layer count.
- **Stats:** for each `host/case/direction`: rounds, rounds with a ≥ 4 s task, swap errors, URL mismatches and unsettled rounds. Also median/max/min plus every value for the longest task, total and maximum Layerize, `pushState`→`before-swap`, click→`page-load`, click→settled and maximum frame gap.

Every round is kept in `rounds[]`, warmup rounds included and marked `measured: false`, so the summary can show the full distribution and its outliers.

## Matrix

| Case | Role | Viewport | Enter (neighbour → target) | Leave |
| --- | --- | --- | --- | --- |
| `en-desktop-compat` | target | 1440×1000 | `/docs/zudo-wind/coming-from-tailwind/` → `/docs/zudo-wind/compatibility/` | reverse |
| `ja-desktop-compat` | control | 1440×1000 | `/ja/docs/zudo-wind/coming-from-tailwind/` → `/ja/docs/zudo-wind/compatibility/` | reverse |
| `en-mobile-compat` | control | 390×844 | as EN desktop | reverse |
| `en-desktop-ordinary` | control | 1440×1000 | `/docs/zudo-wind/overview/` → `/docs/zudo-wind/configuration/` | reverse |

Each sequence runs in a fresh browser context: a hard load of the neighbour, then a soft navigation through an injected `main` link to the target, then back. Every navigation starts only after 1000 ms with no rAF gap above 50 ms, using the same detector as `verify.mjs`. Measurement ends at the next such quiet window, so post-swap Layerize tasks fall inside it. The defaults are 1 warmup round plus 4 measured rounds. Every round runs each host × case once, and the order rotates by round so the hosts and cases interleave.

The 30 s settle and swap budgets are liveness guardrails derived from verify.mjs's 20 s budget (two ≤ 7.5 s stalls plus quiet). Exceeding one marks the round `settled: false` or sets `swapError`; it does not fail the run.

## Commands (manager)

Run from the repo root. Build once; both hosts serve that same `docs/dist`.

```sh
# 1. Build the docs tree once (heavy lane).
bash "$HOME/.claude/scripts/heavy-guard.sh" -- pnpm --filter docs build

# 2a. Plain static host on :4334 (existing no-store static server; no _redirects).
WIND_DOCS_DIST=docs/dist node tests/docs-wind-preview/serve-dist.mjs 4334

# 2b. Actual Cloudflare local host on :4333 (docs/wrangler.toml serves ./dist).
pnpm --dir docs exec wrangler dev --local --port 4333 --ip 127.0.0.1

# 3. Profile both hosts interleaved in one run (Chromium only; CDP tracing).
OUT="research/4166-nav-stall"   # committed summaries; traces stay outside Git
bash "$HOME/.claude/scripts/playwright-guard.sh" -- env \
  NAV_STALL_ORIGINS="static=http://127.0.0.1:4334,wrangler=http://127.0.0.1:4333" \
  NAV_STALL_OUTPUT="$OUT/baseline-$(git rev-parse --short HEAD).json" \
  NAV_STALL_TRACE_DIR="/tmp/zfb-nav-stall/traces-baseline" \
  node tests/docs-navigation-structure/nav-stall-profile.mjs

# 4. After a candidate fix (rebuild, re-serve), compare against the baseline.
bash "$HOME/.claude/scripts/playwright-guard.sh" -- env \
  NAV_STALL_ORIGINS="static=http://127.0.0.1:4334,wrangler=http://127.0.0.1:4333" \
  NAV_STALL_OUTPUT="$OUT/fix-$(git rev-parse --short HEAD).json" \
  NAV_STALL_TRACE_DIR="/tmp/zfb-nav-stall/traces-fix" \
  NAV_STALL_BASELINE="$OUT/baseline-<sha>.json" \
  node tests/docs-navigation-structure/nav-stall-profile.mjs
```

Keep raw traces out of Git. Upload the trace directory as a retained artifact, for example a tarball attached to the tracking issue or a release asset. Record its URL and retention next to the committed summary, and keep the summary readable after the artifact expires.

| Variable | Default | Meaning |
| --- | --- | --- |
| `NAV_STALL_ORIGINS` | unset | `label=origin,...`; interleaves several hosts in one run |
| `DOCS_VERIFY_ORIGIN` / `NAV_STALL_HOST` | `http://127.0.0.1:4333` / `unlabeled` | single host and its label when `NAV_STALL_ORIGINS` is unset |
| `NAV_STALL_OUTPUT` | `/tmp/zfb-nav-stall/summary.json` | summary JSON |
| `NAV_STALL_TRACE_DIR` | `<output dir>/traces` | raw gzipped traces |
| `NAV_STALL_ROUNDS` / `NAV_STALL_WARMUP` | `4` / `1` | measured / discarded rounds |
| `NAV_STALL_CASES` | all four | comma list of case ids |
| `NAV_STALL_REDUCED_MOTION` | `reduce` | `reduce` (as `verify.mjs`) or `no-preference` |
| `NAV_STALL_VIEW_TRANSITIONS` | `native` | `disabled` deletes `document.startViewTransition` before the router loads (router simulation path) |
| `NAV_STALL_EXTRA_CSS` | empty | CSS adopted as a constructable sheet in every document; survives soft swaps |
| `NAV_STALL_TRACE` | `1` | `0` measures without tracing (rules out tracing overhead) |
| `NAV_STALL_TRACE_CPU_PROFILE` | `0` | `1` adds V8 sampling and stacks for JS attribution |
| `NAV_STALL_TRACE_CATEGORIES` | DevTools-like set | override the trace categories |
| `NAV_STALL_CENSUS` / `NAV_STALL_LAYER_TREE` | `1` / `0` | DOM/style census; CDP composited-layer count |
| `NAV_STALL_HEADED` / `NAV_STALL_DPR` | `0` / `1` | headed browser; device scale factor |
| `NAV_STALL_QUIET_MS` | `1000` | quiet window before and after each navigation |
| `NAV_STALL_BASELINE` | unset | earlier summary to compare per `host/case/direction` (labels must match) |

## Predeclared comparison target (#4166)

This target is set before any patch. It is a local comparison, not a CI wall-clock budget.

- **Target case** (`en-desktop-compat`, enter and leave, on each host where the baseline reproduces): the baseline's repeated freeze (`roundsWithFreeze >= 2` of the measured rounds) is gone in the fix run (`roundsWithFreeze == 0`). The median `longestTaskMs` is at most half the baseline median (`medianLongestTaskRatio <= 0.5`). Under `NAV_STALL_BASELINE` the summary's `comparison.rows` computes `baselineRepeatedFreeze`, `freezeEliminated` and `medianLongestTaskHalved`.
- **Controls** (`ja-desktop-compat`, `en-mobile-compat`, `en-desktop-ordinary`): no new page errors and no material regression. The manager judges this from the reported ratios and distributions; the script does not hard-code a regression threshold.
- **Both observations are reported.** A fix that removes the Layerize freeze but leaves the Wrangler `pushStateToBeforeSwapMs` gap closes only observation 1, and the gap stays an open item. The reverse also holds.
- **No reproduction means HOLD.** If the baseline shows no repeated ≥ 4 s freeze on any host, record the matrix and the smallest observed reproduction, name the next environment (other Chrome version, headed, DPR 2, other machine), and hold. Do not ship a fix.

## Candidate causes, ranked from source reading

Sources read: `@takazudo/zfb-runtime@4.3.3` `dist/client-router/{router,events,swap-functions}.js`; `@takazudo/zudo-doc@6.1.1` `dist/{features,content,compiled}.css` and `dist/content/content-table.js`; the host's `docs/src/styles/global.css`.

These facts bear on the ranking:

- Every Markdown table renders as `<div class="overflow-x-auto"><table class="w-full border-collapse …">`. The compatibility page has 23 such tables with about 1,288 rows, and each wrapper is a potential scroll container.
- `features.css` gives the header, desktop sidebar, footer and the two desktop toggles their own `view-transition-name`s through `[data-zfb-transition-persist]`. The root still cross-fades, and under `prefers-reduced-motion: reduce` every snapshot animation is `none`.
- The router commits `pushState` before it calls `document.startViewTransition(cb)`. `zfb:before-swap` is dispatched inside `cb`, and the browser runs `cb` only after it renders and captures the outgoing state. Any capture or old-state rendering cost therefore falls between `pushState` and `before-swap`, the exact window of observation 2.
- #4156 measured EN and JA with matching DOM counts (17,233 nodes, 4,404 `code`, 23 tables, 1,311 rows, 9 layers). Script, layout and style deltas there were tens of ms to about 200 ms, so Layout, style and script are unlikely to be the 4–7 s cost. What differs between the locales is geometry (line wrapping), not structure.

### Observation 1: post-swap Layerize at EN 1440

| Rank | Hypothesis | Why it fits | Cheap experiment |
| --- | --- | --- | --- |
| 1 | **View-transition lifecycle re-layerizes a very large page.** Capturing the new state after the swap, then tearing down the pseudo tree at `finished`, each force a full layerization of a page tall enough to be costly. | It would explain "two long tasks" on entry (new capture plus teardown) and one on leave. | Compare `trace.byPhase` Layerize under `after:zfb:after-swap` and `after:vt-finished` with the VT timestamps in `events`. Re-run with `NAV_STALL_VIEW_TRANSITIONS=disabled`; Layerize gone means VT-driven. Then test only the named chrome: `NAV_STALL_EXTRA_CSS='[data-zfb-transition-persist]{view-transition-name:none!important}'`. |
| 2 | **Paint-chunk/overlap explosion from table scroll wrappers.** At 1440, EN tables overflow their `overflow-x-auto` wrappers because English words cannot break while Japanese can. Many scrollable containers then yield separate scroll-translation and clip property nodes, and Layerize's overlap testing against the sticky or named chrome grows superlinearly. | It is locale-specific through geometry while the DOM matches. | Read `census[…].overflowingTables` and `overflowingScrollContainers` for EN against JA at 1440, and EN at 390. Re-run with `NAV_STALL_EXTRA_CSS='.zd-content .overflow-x-auto{overflow-x:visible}'` (or `overflow:clip`); the freeze vanishing confirms. `NAV_STALL_LAYER_TREE=1` compares composited layer counts. |
| 3 | **Desktop-only chrome layer interaction.** The desktop sidebar is a named VT element with its own scroller, and the sticky header overlays a very tall content chunk list. 390 hides the desktop sidebar, matching the missing stall there. | It is the 1440-only half of the condition. | `NAV_STALL_EXTRA_CSS='#desktop-sidebar{display:none}'` at 1440. A separate run removes the header's stickiness, using a selector taken from the census or the DOM. |
| 4 | **Sidebar island mount after `page-load`.** Mounting `SidebarTree` for the Wind scope changes a large subtree and triggers another layerization. | The second task could follow the island mount. | Check whether the second long `RunTask` starts after `zfb:page-load` with `top` showing script then Layerize; add `NAV_STALL_TRACE_CPU_PROFILE=1`. Compare `census[…].desktopSidebar` sizes EN against JA. |
| 5 | **Raster/paint cost misread as Layerize.** | Lower: #4156 attributed the time to `Layerize` specifically. | Check `trace.topEvents` and per-task `top`. Paint or raster dominating instead refutes Layerize-specific fixes. |

### Observation 2: Wrangler-only `pushState` → swap gap

| Rank | Hypothesis | Why it fits | Cheap experiment |
| --- | --- | --- | --- |
| 1 | **Carry-over work, not host behaviour.** The click landed while the previous page's post-swap Layerize (observation 1) was still pending. Host timing (slower responses, different cache headers) decides whether it lands inside the window. | The recorded blocked task (10063 → 16164 ms) began before `pushState` (10082) and contains `before-swap` (15810), and verify.mjs at that time did not settle before each navigation. | This profiler settles first by default; a gap that disappears under settle-first supports carry-over. Reproduce the no-settle condition with `NAV_STALL_QUIET_MS=0`: each navigation starts as soon as one frame renders, so a leave can land between two post-swap tasks. That also shortens the measured window, so later tasks can fall outside the trace. Compare `preSettle.waitedMs` and the trace long-task `phase`. |
| 2 | **Outgoing-state VT capture.** The old snapshot is rendered and captured between `pushState` and `before-swap`. On a large outgoing page this includes a full Layerize. | It matches the window exactly. | Read `trace.byPhase["after:vt-start"]` (Layerize/Paint) and `timeline.vtStartToUpdateStartMs`. With `NAV_STALL_VIEW_TRANSITIONS=disabled` on Wrangler, a vanished gap confirms capture. |
| 3 | **CPU contention from the local `workerd` process.** Wrangler shares the machine with the renderer, so the main thread can be descheduled. | Wall time could far exceed CPU time. | Compare each long task's `durationMs` and `cpuMs`: `cpuMs` ≪ `durationMs` means descheduling, not page work. Rerunning with Wrangler on another core or machine would also confirm. |
| 4 | **Host-dependent responses change what the router waits on or parses.** Candidates are compression, `cache-control`/revalidation, and stylesheet preloads awaited before the swap. | Only the host differs between runs. | Diff the `assets` `contentEncoding`/`cacheControl`/sha256 between hosts, and `timeline.fetchResponseEndMs` and `documentBytes`. `servedMatchesDist` must be true on both hosts, or the trees differ. |
| 5 | **Tracing or headless artefact.** | Instrumentation can perturb timing. | Re-run with `NAV_STALL_TRACE=0` and with `NAV_STALL_HEADED=1`; a gap that appears or disappears with them is instrumentation-bound. |

After a hypothesis is confirmed, the product fix still goes through the issue's rule: change only the surface the experiment identified, keep content, table readability, navigation, transitions, the sidebar and accessibility intact, and re-measure both entering and leaving against the target above.
