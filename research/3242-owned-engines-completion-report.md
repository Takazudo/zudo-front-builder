# Owned Engines completion report

Evidence in this report is collated against merged base commit `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` on 2026-09-29. The release build and same-host measurements were run at that commit; case results cite their tested commits, hosts, and commands. No new Rust or browser suite was run for this report.

## Implemented

- Replaced the Tailwind utility executable with the owned `zudo-wind` compiler, closed catalog, candidate/source handling, token and manifest configuration, diagnostics, explain/audit commands, authored CSS pipeline, and build/dev integration.
- Added the owned `zudo-react` runtime inside `@takazudo/zfb`, with core, JSX, JSX development, server, and client entries; wired SSR, islands, hydration, forms, signals, scopes, structural regions, and runtime identity into zfb.
- Migrated the basic-blog and node-free scaffolds, package exports, MD/MDX output, and the owned-engine docs. Removed the runtime selectors, aliases, binaries, and production dependencies owned by the cutover tasks.
- The docs host remains on its published 2.x stack by the epic decision. Its v3 migration is tracked separately in [#3329](https://github.com/Takazudo/zudo-front-builder/issues/3329), after the public zudo-doc migration in [#3328](https://github.com/Takazudo/zudo-front-builder/issues/3328).

## Verified

### Evidence levels and tiers

The case table uses repository levels: L1 unit/logic, L3 build output, L4 real process/browser, and L5 computed styles. T0 is the inner loop, T1 is the required PR gate, T3 is scheduled re-exam, and T4 is the local heavy lane. A path-filtered, non-required browser workflow is named as such rather than promoted to T1.

### Wind cases

| Case / owner | Level / tier | Command or run; host; tested commit | Result and limit |
| --- | --- | --- | --- |
| W-A01 / [#3257](https://github.com/Takazudo/zudo-front-builder/issues/3257) | L1 and L5 / T4; optional path-filtered workflow | Guarded `pnpm test:wind-computed-style`; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `35ef2c78846aaf23b267fceade59015493ca5d94` | PASS in the 10-test suite: deterministic CSS/explain output and structured invalid-candidate diagnostics. No real zfb build in this suite. |
| W-A02 / [#3257](https://github.com/Takazudo/zudo-front-builder/issues/3257) | L5 / T4; optional path-filtered workflow | Same guarded `pnpm test:wind-computed-style`, host, and commit as W-A01 | PASS: padding, margin, and gap permutations have the specified computed values. Chromium only. |
| W-A03 / [#3257](https://github.com/Takazudo/zudo-front-builder/issues/3257) | L5 / T4; optional path-filtered workflow | Same guarded `pnpm test:wind-computed-style`, host, and commit as W-A01 | PASS: breakpoint thresholds, hover capability, focus-visible, dark matching, layer order, and rejected variant combination. No Firefox/WebKit or other host. |
| W-A04 / [#3269](https://github.com/Takazudo/zudo-front-builder/issues/3269) | L4 / T1 | `bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo test -p zfb --test wind_clean_warm_identity_e2e -- --nocapture`; Darwin 25.6.0 arm64; `154afbb88500b699aca3dcf274a5338d4e67bbda` | PASS, no skip: duplicate ownership, safelist, manifest replacement/removal, file add/delete/rename, and clean/warm CSS identity (623 bytes). Windows watcher behavior was not exercised. |
| W-A05 / [#3269](https://github.com/Takazudo/zudo-front-builder/issues/3269) | L3 / T1 | `ZFB_WIND_REAL_BUILD_DIST=/tmp/zfb-3242-reviews/3269-browser-dist bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo test -p zfb --test wind_real_build_confirm_build -- --nocapture`; Darwin 25.6.0 arm64; `154afbb88500b699aca3dcf274a5338d4e67bbda` | PASS, no skip: client-only class literal, authored class, dynamic-class audit origins, and generated CSS assertions. No pixel comparison. |
| W-A06 / [#3269](https://github.com/Takazudo/zudo-front-builder/issues/3269) | L3 / T1 and L4 / T4 | Guarded `ZFB_WIND_REAL_BUILD_DIST=/tmp/zfb-3242-reviews/3269-browser-dist cargo test -p zfb --test wind_real_build_confirm_build -- --nocapture` and guarded `pnpm exec playwright test --config tests/wind-real-build/playwright.config.mjs`; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `742e81846dea8bad048d20689cc2ac9f78ba0cbb` / merged `154afbb88500b699aca3dcf274a5338d4e67bbda` | PASS: nested/package/project assets and CSS Modules/global CSS were emitted; local browser requests had expected status and MIME. The browser network suite is local, not a CI gate. |
| W-A07 / [#3257](https://github.com/Takazudo/zudo-front-builder/issues/3257), [#3269](https://github.com/Takazudo/zudo-front-builder/issues/3269) | L5 / T4 and L3 / T1 | Guarded `pnpm test:wind-computed-style` at `35ef2c78846aaf23b267fceade59015493ca5d94`; guarded `cargo test -p zfb --test wind_real_build_confirm_build -- --nocapture` at `154afbb88500b699aca3dcf274a5338d4e67bbda`; macOS 26.6.1 arm64 / Darwin 25.6.0 arm64 as applicable | PASS: reset variants, controls, headings/lists, focus outline and bare border were checked by computed style; CSS Modules and authored global CSS were published. No post-change screenshot/pixel review was performed. |
| W-A08 / [#3291](https://github.com/Takazudo/zudo-front-builder/issues/3291) | L3 and L4 / T4; current scaffold evidence T1 | Guarded `node tests/built-site-smoke/verification/observe-build.mjs target/release/zfb`; macOS 26.6.1 arm64; `464d3aad8b95c3fc11bee1fae0e9906264103c8a`. Current packed artifact: [node-free-smoke run 36503194558](https://github.com/Takazudo/zudo-front-builder/actions/runs/36503194558), Linux x64 build at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | PASS: positive esbuild process observation, nonempty stylesheet, and no observed Tailwind child. Artifact inspection also passed as detailed below. Process polling can miss a short-lived child; Windows/macOS x64 distribution is unverified. |

### zudo-react cases

| Case / owner | Level / tier | Command or run; host; tested commit | Result and limit |
| --- | --- | --- | --- |
| R-A01 / [#3275](https://github.com/Takazudo/zudo-front-builder/issues/3275) | L1 / T0, with packed API check | `pnpm --filter @takazudo/zfb test`; `node packages/zfb/scripts/zudo-react-packed.mjs check`; Darwin 25.6.0 arm64; `4e179273dabed1e883f2bf039d5c46e57ddfd143` | PASS: deterministic server HTML, request isolation, escaping, SVG/boolean/ARIA rules, and no server activation. The packed script's optional esbuild probe was skipped; it is not counted as proof here. |
| R-A02 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280), [#3291](https://github.com/Takazudo/zudo-front-builder/issues/3291) | L3 and L4 / T4; browser job is non-required | Guarded `pnpm test:zudo-react-browser` and built-site Playwright suite; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` / `464d3aad8b95c3fc11bee1fae0e9906264103c8a` | PASS: adjacent/empty text, fragments, two roots, and held real-build DOM nodes retain identity. Chromium only. |
| R-A03 / [#3275](https://github.com/Takazudo/zudo-front-builder/issues/3275), [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280) | L1, L4, and L5 / T0 and T4 | Package logic tests and guarded browser suite; Darwin 25.6.0 arm64 / macOS 26.6.1 arm64; `4e179273dabed1e883f2bf039d5c46e57ddfd143` / `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` | PASS: signal/computed updates, snapshot reads, setup-once, dependencies, batch/flush, and hidden-panel computed style using authored CSS. The panel case does not use zudo-wind utility CSS. |
| R-A04 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280), [#3291](https://github.com/Takazudo/zudo-front-builder/issues/3291) | L4 / T4; browser job is non-required | Guarded `pnpm test:zudo-react-browser` and `pnpm exec playwright test --config tests/built-site-smoke/playwright.config.mjs --retries=0`; macOS 26.6.1 arm64; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` / `464d3aad8b95c3fc11bee1fae0e9906264103c8a` | PASS: native event delivery, ref-before-activation, duplicate activation handling, and canceled async work. Chromium only. |
| R-A05 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280) | L4 / T4; browser job is non-required | Guarded `pnpm test:zudo-react-browser`; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` | PASS: dirty DOM/focus survives hydration; text, textarea, checkbox, single-select and radio models reconcile; reset policy passes. Multiple select is explicitly rejected. |
| R-A06 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280) | L4 / T4, synthetic only | Guarded `pnpm test:zudo-react-browser`; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` | Synthetic composition/input events pass and preserve composing input plus equal-value selection. **R-A06 is synthetic only.** A real Japanese IME pass is deferred. |
| R-A07 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280), [#3291](https://github.com/Takazudo/zudo-front-builder/issues/3291) | L4 / T4; browser job is non-required | Guarded `pnpm test:zudo-react-browser` and `pnpm exec playwright test --config tests/built-site-smoke/playwright.config.mjs --retries=0`; macOS 26.6.1 arm64; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` / `464d3aad8b95c3fc11bee1fae0e9906264103c8a` | PASS: cleanup, repeated disposal, deferred-island removal, failed preflight, and real-build lifecycle assertions. Chromium only. |
| R-A08 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280) | L4 / T4; browser job is non-required | Guarded `pnpm test:zudo-react-browser`; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` | PASS: malformed/mismatched roots fail closed with edited DOM preserved and neighboring roots independent. Chromium only. |
| R-A09 / [#3291](https://github.com/Takazudo/zudo-front-builder/issues/3291) | L4 / T4; browser job is non-required | Guarded `pnpm exec playwright test --config tests/built-site-smoke/playwright.config.mjs --retries=0`; macOS 26.6.1 arm64; `464d3aad8b95c3fc11bee1fae0e9906264103c8a` | PASS in three consecutive default runs: disposable, persisted, changed-props and browser-Back lifecycles. Chromium only. |
| R-A10 / [#3280](https://github.com/Takazudo/zudo-front-builder/issues/3280) | L4 / T4; browser job is non-required | Guarded `pnpm test:zudo-react-browser`; macOS 26.6.1 arm64, Chrome for Testing 149.0.7827.55; `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` | PASS: conditional regions, keyed reorder, focus/caret retention, remove/re-add, and transactional duplicate-key rejection. Chromium only. |
| R-A11 / [#3291](https://github.com/Takazudo/zudo-front-builder/issues/3291) | L3 and L4 / T4; browser job is non-required | Guarded real build, browser suite, and `bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo nextest run -p zfb --test build_package_routes --test html_minify_build --no-capture`; macOS 26.6.1 arm64; `464d3aad8b95c3fc11bee1fae0e9906264103c8a` | PASS: emitted runtime definition count, duplicate-runtime negative control, two islands sharing a live signal, and minified HTML path. A currently published migrated preset is not covered; see [#3331](https://github.com/Takazudo/zudo-front-builder/issues/3331). |

### Current-head CI and clean-room artifact

- [Health run 36503194590](https://github.com/Takazudo/zudo-front-builder/actions/runs/36503194590) completed successfully at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64`, including the required `health` job and the `wasm-md browser` job.
- [Node-free smoke run 36503194558](https://github.com/Takazudo/zudo-front-builder/actions/runs/36503194558) completed successfully at the same commit. The packed `Scaffold E2E (packed tarballs, pre-publish)` job and built-site browser job passed. The four node-free jobs passed: amd64 JSON, arm64 JSON, amd64 TypeScript config, and arm64 TypeScript config.
- I downloaded the run's `create-zfb-showcase-dist` artifact with `gh run download` and inspected the Linux x64-built files on macOS 26.6.1 arm64. The stylesheet `assets/styles-b4f6ed9e.css` and islands bundle `assets/islands-a26ddfe3.js` are nonempty. The stylesheet search for Tailwind directives/imports returned no matches. `data-zfb-island="ThemeToggle"` is present on every emitted basic-blog page. Searches for `preact-render-to-string`, `preact/hooks`, `react/jsx-runtime`, `react-dom`, and `tailwindcss` returned no file matches; the artifact has no matching runtime filenames. This inspection proves emitted content, not interaction of the basic-blog `ThemeToggle` island.

### Same-host release measurements

The pre-change values below are from [the baseline note](3242-owned-engines-baseline.md) at `704f42c87703e4f62eb4eeaf1758a0af557520f3`; current values are from the guarded release build and five-sample measurement helper at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64`. Both runs used macOS 26.6.1 (Darwin 25.6.0), arm64 Apple A18 Pro, Rust 1.94.0, Node 24.14.0, pnpm 11.3.0, and the default embedded-node-modules cache. The release build command was `bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo build --release -p zfb`; it passed at the current commit. Size rows are from the five-sample measurement helper used for the baseline; emitted site sizes were measured after the listed release-binary build. Timing rows use fresh basic-blog copies for cold runs and a repeat build in the same directory for warm runs. Values are measurements only; no performance target was set.

| Measurement and command | Baseline `704f42c87703e4f62eb4eeaf1758a0af557520f3` | Current `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | Comparability |
| --- | ---: | ---: | --- |
| Release binary; `stat -f '%N %z' target/release/zfb` after the guarded release build | 175,768,640 B | 99,346,992 B | Same host and command. Current `zfb --version` reports embedded esbuild only. |
| Embedded vendor `@takazudo/`; five-sample helper | 367,933 B | 471,313 B | Same host; owned SDK tree is different content. |
| Embedded vendor `bin/`; five-sample helper | 86,490,962 B | 9,934,834 B | Same host; current directory contains esbuild only. |
| Embedded vendor `hono/`; five-sample helper | 1,416,507 B | 1,416,507 B | Same host and package. |
| Embedded vendor `preact/`; five-sample helper | 644,857 B | absent | Removed from the embedded tree. |
| Embedded vendor `preact-render-to-string/`; five-sample helper | 305,769 B | absent | Removed from the embedded tree. |
| Embedded `bin/esbuild`; five-sample helper | 9,934,834 B | 9,934,834 B | Same pinned esbuild bytes. |
| Embedded `bin/tailwindcss-v4`; five-sample helper | 76,556,128 B | absent | Tailwind executable removed. |
| Staged `crates/zfb/binaries/esbuild/esbuild`; `find crates/zfb/binaries -type f -not -name '*.md' -not -name '.gitkeep' -exec stat -f '%N %z' {} \;` | 9,934,834 B | 9,934,834 B | Same pinned esbuild bytes. |
| Staged Tailwind binary slot; same `find`/`stat` command | 76,556,128 B | absent | Tailwind binary slot removed. |
| Basic-blog `dist/` total; five-sample helper after warm build | 96,016 B | 117,278 B | Same host and file count, but template content, route slug, CSS and runtime changed. No size claim follows from this comparison. |
| Basic-blog emitted CSS; `stat -f '%N %z'` on the built `assets/styles-*.css` | 28,090 B | 21,416 B | Same host; authored stylesheet and engine output changed. |
| Basic-blog islands bundle; `stat -f '%N %z'` on the built `assets/islands-*.js` | 18,880 B | 48,865 B | Same host; owned runtime is new bundle content. |
| Built-site-smoke fixture `dist/` total; release-binary `zfb build` then output-size summary | 18,833 B | 80,055 B | Not comparable: the baseline fixture emitted one HTML page; the current fixture emits five and exercises more runtime cases. |
| Built-site-smoke islands bundle; output-size summary after `zfb build` | 18,108 B | 72,950 B | Not comparable for the same fixture-scope change. |

Cold and warm build wall times are medians/minimums/maximums across five samples. The command for each sample was `cd <fresh basic-blog copy> && ZFB_BUILD_TIMING=1 <release zfb> build`; host and revisions are the two values stated above.

| Build kind | Baseline median / min / max (ms) | Current median / min / max (ms) |
| --- | ---: | ---: |
| Cold | 6,992.691 / 5,761.533 / 13,504.591 | 5,149.210 / 4,265.185 / 5,671.371 |
| Warm | 6,638.351 / 5,841.855 / 10,464.120 | 4,292.371 / 2,819.439 / 5,176.720 |

The timing log keeps separate phase medians. It uses the same per-sample build command, host and commits as the wall-time table.

| Phase median (ms) | Baseline cold | Current cold | Baseline warm | Current warm |
| --- | ---: | ---: | ---: | ---: |
| `vendor-extraction-and-bundler-input` | 718 | 968 | 601 | 727 |
| `main-esbuild-bundle` | 577 | 650 | 508 | 450 |
| `v8-paths-eval` | 30 | 44 | 43 | 40 |
| `css` | not separately reported | 60 | not separately reported | 50 |
| `production-assets` | 3,888 | 2,047 | 3,925 | 1,712 |
| `v8-boot-and-render` | 157 | 98 | 167 | 119 |

The baseline emitted no separate `css` timing phase; its CSS work is included in the surrounding phase output. Phase boundaries therefore differ. The baseline cold samples also began with prior Tailwind warm-up markers present. Build times are not interpreted as a speed result. The `built-site-smoke` fixture size comparison is not comparable because the fixture grew from one emitted page to five.

### Consumer acceptance

Basic-blog page builds are present in the current packed artifact; the node-free routes are covered by the four current workflow smoke jobs. The R cases use their named owned-runtime fixtures; they do not imply every scaffold page or island received a separate browser interaction test.

| Consumer page | Cases and evidence | Intentional change | Remaining gap |
| --- | --- | --- | --- |
| basic-blog `/` | W-A08; current packed artifact at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | Explicit palette and owned reset; new utility output | No pixel review; theme toggle interaction not tested in this page. |
| basic-blog `/404` | W-A08; current packed artifact at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | Owned reset and CSS utility catalog | No pixel review. |
| basic-blog `/about` | W-A08; current packed artifact at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | Owned reset and CSS utility catalog | No pixel review. |
| basic-blog `/blog/hello-zfb` | W-A08; current packed artifact at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | MDX emits the owned JSX runtime dialect | This route was built; its MDX island/browser interaction was not separately tested. |
| basic-blog `/blog/markdown-showcase` | W-A08; current packed artifact at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | Owned MDX and CSS output | No per-feature visual review. |
| basic-blog `/blog/styling-with-zudo-wind` | W-A08; current packed artifact at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | Styling article slug and style examples migrated | Baseline route was `styling-with-tailwind`; page pixels were not compared. |
| basic-blog `ThemeToggle` island (shared layout on the pages above) | W-A08 marker in packed artifact; R-A01–R-A11 cover runtime contracts in isolated fixtures | Uses zudo-react signals, owned attributes and island markers | The real template island was not clicked in the packed artifact. |
| node-free `/` | Four current node-free CI smoke jobs at run 36503194558 | JSON and TypeScript config paths use the owned runtime; no Node is present in the smoke container | No visual review. |
| node-free `/about` | Same four current node-free CI smoke jobs | Owned server render | No individual route browser interaction. |
| node-free `/posts/hello` | Same four current node-free CI smoke jobs | Owned MD rendering | No individual route browser interaction. |
| node-free `/posts/second` | Same four current node-free CI smoke jobs | Owned MD rendering | No individual route browser interaction. |
| built-site-smoke `/` | R-A02, R-A04; built-site browser job in run 36503194558 | Island hydration uses the owned runtime | Chromium only. |
| built-site-smoke `/identity` | R-A02, R-A07, R-A08 | Strict identity, markers, fallback replacement and fail-closed adoption | Chromium only. |
| built-site-smoke `/nav-a` | R-A04, R-A07, R-A09 | Disposable and persisted island ownership | Chromium only. |
| built-site-smoke `/nav-b` | R-A09 | Equal roots persist; changed props follow recreate policy | Chromium only. |
| built-site-smoke `/shared-state` | R-A11 | Shared signal identity across emitted island modules | A published preset consumer is not included. |
| built-site-smoke `Counter` island | R-A02/R-A04 family plus current built-site browser smoke | Owned signal and native event handling | The browser smoke is a fixture proof, not the basic-blog `ThemeToggle`. |
| built-site-smoke `Gallery` island | R-A02/R-A04 family; current built-site browser smoke | Nested component and client bundle import | Chromium only. |
| built-site-smoke `/identity` `IdentityProbe` first root | R-A02/R-A08 | Strict identity and existing-DOM adoption | Chromium only. |
| built-site-smoke `/identity` `IdentityProbe` second root | R-A02/R-A08 | Independent root adoption | Chromium only. |
| built-site-smoke `SkipSsrProbe` | R-A08 | Skip-SSR fallback is replaced after successful mount | Chromium only. |
| built-site-smoke `DeferredProbe` | R-A07 | Removed deferred roots do not activate later | Chromium only. |
| built-site-smoke `/nav-a` `DisposableProbe` | R-A07/R-A09 | Disposal and DOM removal have separate behavior | Chromium only. |
| built-site-smoke `/nav-a` `EqualProbe` | R-A09 | Unchanged persisted root keeps its identity | Chromium only. |
| built-site-smoke `/nav-a` `ChangedProbe` | R-A09 | Changed props recreate a render-mode root | Chromium only. |
| built-site-smoke `/nav-b` `EqualProbe` | R-A09 | Unchanged persisted root keeps its identity after Back | Chromium only. |
| built-site-smoke `/nav-b` `ChangedProbe` | R-A09 | Changed props recreate a render-mode root after Back | Chromium only. |
| built-site-smoke `/shared-state` `SharedWriter` island | R-A11 | Writer uses the emitted owned reactive runtime | No published preset consumer. |
| built-site-smoke `/shared-state` `SharedReader` island | R-A11 | Reader shares the writer's runtime identity | No published preset consumer. |

### Documentation drift and resolution modes

[#3309](https://github.com/Takazudo/zudo-front-builder/issues/3309) reconciled the changed English and Japanese pages against the implementation, both package export maps, config source, CLI source, catalog and committed fixtures. Its final review records `node docs/scripts/generate-wind-reference.mjs --locale en --check` and the corresponding `--locale ja --check` as passing on the merged tree at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64`; the review does not record the host for those checks. Stub-marker, export/specifier, changed-page heading/link, docs package, docs build, HTML, and island checks passed in that task. The v3 sample code was source-checked; it was not compiled, type-checked, or hydrated by a v3 docs host.

The known API drift is `islandRootType`: the implementation and docs expose it from the server entry, while the frozen zudo-react v1 contract lists only `renderToString`, `islandRoot`, and `serializeProps`. This is an open contract defect tracked by [#3327](https://github.com/Takazudo/zudo-front-builder/issues/3327), not an intentional amendment to the contract. The docs host is still on published 2.x and has no v3 host validation.

| Resolution mode | Proof and command | Host / commit | Result and limit |
| --- | --- | --- | --- |
| Workspace source | #3275 package suite and type fixtures; `pnpm --filter @takazudo/zfb test` | Darwin 25.6.0 arm64; `4e179273dabed1e883f2bf039d5c46e57ddfd143` | PASS: source package SSR and logic assertions. Real zfb embedded resolution is not implied by source tests. |
| Packed tarball | #3275 `node packages/zfb/scripts/zudo-react-packed.mjs check`; #3280 `pnpm test:zudo-react-browser` builds the fixture against the staged packed SDK | Darwin 25.6.0 arm64 for package check; macOS 26.6.1 arm64 / Chrome for Testing 149.0.7827.55 for browser run; `4e179273dabed1e883f2bf039d5c46e57ddfd143` / `6ae3a75b8a6176da38b5d4d9e2c0da822e88c9b0` | PASS: packed production/development types and real packed-runtime browser tests. The optional esbuild probe in the earlier packed script was skipped; the later browser fixture independently executes the packed runtime. |
| Embedded tree without `node_modules` | #3282 embedded no-`node_modules` resolution 3/3; #3289 `framework_packages_no_pnpm` 3/3; four current node-free smoke jobs in run 36503194558 | Worker host is not recorded in the #3282/#3289 comments; CI Linux amd64 and arm64 jobs ran at `cd78dc1000627bb079f81f24a2a5339e2ff3fe64` | PASS: embedded owned entries resolve without project `node_modules`; the four JSON/TypeScript amd64/arm64 smoke jobs all passed. |

### Review-target residue and seam search

The #3308 searches were rerun with the issue's exact source patterns on this report worktree atop `cd78dc1000627bb079f81f24a2a5339e2ff3fe64`. Search A returned 124 source lines, Search B returned 198 files including this report, and Search C returned these five paths:

- `crates/zfb-content/assets/syntaxes/typescript/TypeScriptReact.sublime-syntax`
- `docs/src/content/docs-ja/zudo-react/coming-from-preact-hooks.mdx`
- `docs/src/content/docs-ja/zudo-wind/coming-from-tailwind.mdx`
- `docs/src/content/docs/zudo-react/coming-from-preact-hooks.mdx`
- `docs/src/content/docs/zudo-wind/coming-from-tailwind.mdx`

Search B's one-file increase from #3308 is this report's migration evidence and is classified as `historical`. Search C is unchanged because the report filename contains none of the search terms.

The classified residue list from #3308 is:

| Class | What remains |
| --- | --- |
| `historical` | Changelogs, research notes, the protected `crates/CLAUDE.md` provenance ledger, Astro client-router port spec, dated lesson/nextest records, and the quoted Tailwind Sibling Source epic title. |
| `frozen-docs` | Docs-host files outside content pages, including host config/skills and `DEPENDENCIES.md`'s published 2.x keep-list. |
| `lockfile-docs` | Dependency entries owned by the frozen docs importer. |
| `owned-elsewhere` | Current docs content pages (#3309), repository prose/manifests (#3292), and shipped-size files (#3307). |
| `literal-sample` | `crates/zfb-content/tests/fixtures/mdx-real/`, the MDX fallback sample, and `crates/zudo-wind/tests/fixtures/extract/styling.md`; fixture contents stayed unchanged. |
| `external-name` | TypeScript `react-jsx`/`react-jsxdev` mode values, SWC `transforms::react`/`ReactOptions`, the TypeScriptReact grammar, external JSX specifiers, and intentional negative assertions that legacy imports stay absent. |
| `migration-diagnostic` | Removed-key errors, leftover-directive detection, ZW009 tests, `jsxRuntime` rejection, and negative tests for old artifacts/imports. |
| `third-party-sample` | Third-party package names in `crates/zfb/src/commands/new.rs` and `crates/zudo-wind/tests/fixtures/extract/*.tsx`; production does not special-case them. |
| `false-positive` | English “react to”, “reacts”, and “reactor” matches. |

The four planner-addendum comment-only paths were also reconciled in #3308; the final classified list reports no unowned residue.

The #3308 wording sweep changed no user-visible error or help text, as recorded in its final review report.

The seam search used the exact names from #3282, #3283, and ZR21: `framework_esbuild_flags`, `FrameworkAdapter`, `JsxDialect`, `ReactCompat`, `ZudoReact`, `@takazudo/zfb/jsx-factory`, `jsx-factory.ts`, `FrameworkKind`, `JsxRuntime`, and the old React alias selectors. The exact-name search returns one test-only match: `crates/zfb-islands/src/esbuild.rs:5106` asserts that the old factory alias is absent. No callable selector, seam type, or production alias path remains.

## Intentional changes

- After the cutover, an absent `wind` key enables an empty v1 config with reset `none`, no implicit palette, and no implicit spacing unit. The basic-blog scaffold declares its tokens and selects `owned-v1`; `wind: false` disables owned utility generation while authored CSS and highlight processing remain available.
- Utilities use the closed v1 catalog, configured breakpoints/tokens, bounded variant grammar, strict explicit entries, producer-keyed manifests, and unlayered emission after the leading layer prelude. Build/dev source discovery is limited to `.tsx`, `.ts`, `.jsx`, `.js`, `.mdx`, and `.md`; standalone explicit CSS sources can also name `.html` and `.mjs`.
- Utility CSS is not passed through a new final minifier. `space-x/y` and `divide-x/y` use direct visible sibling pairs. Ring, animation, scale, and transform-string utilities move to authored CSS or supported outline/rotate/translate forms.
- zudo-react uses setup-once components, branded writable signals for models, explicit scope effects, native `on:event` listeners, HTML/CSS spellings, strict JSON island props, identity checks, and fail-closed per-island hydration. DOM state wins at hydration. A changed component or props value recreates a render-mode root; an unchanged persisted root remains.
- The v1 runtime is deliberately bounded. It does not promise React/Preact compatibility or matching legacy output.

## Unsupported

The frozen specs, not historical Tailwind/React expectations, define these limits.

- zudo-wind rejects named group/peer markers, attribute and arbitrary-selector variants, arbitrary properties, stacked variants, important prefixes/suffixes, URL functions, unconfigured tokens/breakpoints, and recognized unsupported utility families. It has no implicit palette, spacing scale, or named tokens. Tailwind directives, Tailwind theme functions, and plugin/config directives are not accepted.
- zudo-wind does not provide ring, animation/keyframe, scale, or transform-string utilities. Use authored CSS or supported outline/rotate/translate entries. Before/after variants do not insert `content`.
- zudo-react rejects React/Preact prop spellings and runtime aliases. It does not provide hooks, context, portals, class components, streaming, lazy/Suspense, callback refs, or generalized tree reconciliation. It offers `flattenChildren`; no `cloneElement` compatibility API is part of the contract.
- Form models support text, textarea, checkbox, single select, and radio. Multiple-select models, file/contenteditable models, numeric/date/range models, reactive option text, and dynamic option/control-shape changes are unsupported. Islands require a single registered component child and strict JSON props.
- Browser claims here are Chromium claims. Firefox, WebKit, Windows, and macOS x64 distribution are unverified.

## Unfinished or unverified

- `manual_ime: deferred` — [#3330](https://github.com/Takazudo/zudo-front-builder/issues/3330) contains the exact headed-browser steps for a human using a real Japanese IME. R-A06 is synthetic only; no sentence in this report treats the manual pass as successful.
- Pixel comparison against the pre-change screenshots is unverified. The reference screenshots are linked from the [#3245 comment](https://github.com/Takazudo/zudo-front-builder/issues/3245); this report did not launch a browser or produce post-change screenshots. Computed-style assertions passed where the case table says L5.
- The docs host remains on published zfb 2.20.2 and zudo-doc 5.27.0. The public zudo-doc port is tracked in [#3328](https://github.com/Takazudo/zudo-front-builder/issues/3328); re-pin and port the host after publication in [#3329](https://github.com/Takazudo/zudo-front-builder/issues/3329). A published-preset regression fixture is tracked in [#3331](https://github.com/Takazudo/zudo-front-builder/issues/3331).
- Four agent-found fixes remain open for separate PRs against `main` after the root PR merges: [#3321](https://github.com/Takazudo/zudo-front-builder/issues/3321) fixes a stale node-free README key; [#3325](https://github.com/Takazudo/zudo-front-builder/issues/3325) validates reactive `rawHtml` updates; [#3326](https://github.com/Takazudo/zudo-front-builder/issues/3326) corrects the public no-pipeline MDX prop spelling; [#3327](https://github.com/Takazudo/zudo-front-builder/issues/3327) removes the extra `islandRootType` server export and updates both API references. The current head has not received those changes, and this report does not claim them fixed.
- The md-wasm size manifest is measured with a pre-release label. Release builds stamp `ZFB_RELEASE_VERSION` into each wasm artifact; this changes all four SHA-256 digests, and byte-exact `finalWasm` validation can fail under the shorter breaking-major version string. Re-measure `crates/zfb-md-wasm/shipped-sizes.json` from a CI run carrying the release version before cutting the release, as recorded in [#3307](https://github.com/Takazudo/zudo-front-builder/issues/3307).
- The docs source review did not compile or type-check v3 samples through the frozen 2.x docs host. No post-release published-package validation is claimed. No performance target was approved.

## Removed

- The Tailwind executable download, version pin, warm-up path, environment overrides, binary slot, build embedding, and Tailwind process lane were removed. `zfb --version` now reports the release version and embedded esbuild only.
- Preact/React runtime selection, adapters, selector types, JSX aliases, and production package dependencies were removed from the zfb engine path. The md-wasm `jsxRuntime` option/type and the old public runtime selector were removed. The frozen docs host still uses its published 2.x dependency graph until its re-pin issue is done.
- Temporary factory and emitter selectors from #3282/#3283 were removed by the cutover. The search above distinguishes test-only negative assertions from a callable runtime seam.
- The old published-preset identity fixture was removed because its pinned Preact-era preset could not run against the owned runtime. Coverage will return as the published-preset fixture in [#3331](https://github.com/Takazudo/zudo-front-builder/issues/3331).

The report index row already exists in `research/README.md` from #3244. This file contains the only change in this worktree.
