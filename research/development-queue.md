# Development checkpoint and qualification queue

This ledger applies the [canonical policy](../CLAUDE.md#moving-development-and-frozen-qualification).
It is not release approval, a scheduler, or a replacement for source-bound receipts.

## Linux CI browser provisioning — 2026-11-07 UTC

- Checkpoint: main `6e93de58c89c204a6643024f159a61430d022901` (PR #4605 merged;
  v5.2.2 published from `282551e6e59e535698bb2bcbbf15b967d672743d`). Zero open
  PRs at start.
- Original outcomes, preserved as **cancelled**, not failed tests and not
  passes: health run 42214732419 job 127881880511 (ubuntu-24.04 image
  20261101.360.1) hit its 90-minute limit in "Collect Linux x64 island-size
  measurements"; budget enforcement, its artifact and `wasm-md browser` did
  not run. Chromium preparation took 9m34s (APT 21.1 MB in 9m08s) and full-Wind
  preparation 48m39s (APT 87.4 MB in 48m11s at 30.2 kB/s), all package
  archives from azure.archive.ubuntu.com; index metadata and browser archives
  were fast. Smoke run 42214732314 job 127884360375 (ubuntu-latest, observed
  26.04 image 20261101.186.1) hit its 10-minute limit downloading fonts; the
  fixture build and browser suite never started. In that same run Scaffold E2E
  installed the same Chromium dependencies in 23 s, and the showcase and docs
  deployments succeeded. Healthy main controls: run 42172565682 (Chromium 20 s,
  full 26 s), 42112172001 (20 s / 35 s), 42093017236 (26 s / 2m31s).
- Repair (branch `base/ci-browser-provisioning-20261107`):
  `scripts/install-playwright-linux.mjs` replaces `playwright install
  --with-deps` in health's Chromium and Wind stages, `wasm-md browser` and the
  Built-site L4 job. Packages come from Playwright 1.61.0's own dry-run;
  acquisition (`apt-get update`, resolve, `--download-only`) is bounded by
  elapsed time with at most one retry, only after all apt processes exit,
  through an invocation-owned APT_CONFIG that carries only the runner's enabled
  Ubuntu stanzas, pointed at archive/security.ubuntu.com (fields and signing
  unchanged; other sources left out), and is confirmed with `apt-config dump`.
  Only a timeout or apt's own fetch/connection wording is retried.
  Installation (`--no-download`) is never retried. Readiness requires a clean
  dry-run, the registry executables and a headless launch per engine. Each
  invocation has a total cap and writes a JSON receipt uploaded with
  `if: always()`. Built-site L4 and `wasm-md browser` are pinned to
  ubuntu-24.04. No runtime,
  package, lockfile, browser revision, Wind profile, size budget or required
  check changed.
- Verification owner: this repair's manager. Source-bound receipts (PR checks,
  the `wind_full=true` health dispatch on the frozen head, Built-site L4,
  `wasm-md browser`, post-merge main runs) are recorded on the PR, not here.

## Submit-button descendant modifiers — 2026-11-05 UTC

### Continuation receipt — 2026-11-05 UTC

- Completed first run at `745008ccb215fec2f7b56922ac43b7e2261166d7`: guard
  **FAIL**, exit 1, 186 seconds, minimum memory 6065 MB; **94 passed, 12 failed,
  0 skipped, 0 interrupted, 0 unexecuted**, retries 0. Four failures were
  empty form target and empty submitter target, router and native oracle, each
  waiting for a popup at form-submit line 243. Eight were anchor/area named base
  and first target base, router and native oracle, comparing history length 2
  against observed 3 at link-intent line 81. Original error contexts and last-run
  report preserved in `~/zfb-native-evidence/submit-modifier-first-745008c/`;
  original console and guard result recovered in the session. No retry traces
  were configured. All 15 modifier cases passed in that first run.
- Diagnostic on the same M4 established that WebKit's empty-target form in
  both modes navigates the original document with one GET and correct payload,
  no popup and no router preparation. The test incorrectly expected Chromium's
  popup behavior. WebKit's named-frame navigation increased joint history length
  in both modes. The harness now independently observes native destination and
  named-frame history, keeps native navigation distinct from SPA ownership, and
  checks document request, payload, one submission and page errors. Existing
  `data-zfb-reload` oracles initialize the router; modifier oracles stub it.
  No runtime change. Independent read-only review found no remaining actionable
  defect after its assertion findings were addressed.
- Corrected frozen executable source `80c0fdd8ca63df3511fd5446c10f27b30865128a`:
  native M4 WebKit **106 passed, 0 failed, 0 skipped**, retries 0, guard PASS,
  exit 0, 58 seconds, minimum memory 6734 MB. Host Mac16,12 Apple M4, arm64,
  translation 0, macOS 27.0.1 (26A434), Node 24.13.0, pnpm 12.8.2,
  Playwright 1.61.0, WebKit 26.5 revision 2311. Clean committed source at run
  start. Three-spec command used `bash "$HOME/.codex/scripts/heavy-guard.sh" --
  env ZFB_BROWSER_ENGINE=webkit pnpm exec playwright test --config
  tests/router-chromium/playwright.config.mjs form-modifier.chromium.spec.mjs
  form-submit.chromium.spec.mjs link-intent.chromium.spec.mjs --project=webkit
  --workers=1 --retries=0`. Affected selection also passed 12/12 on each of
  WebKit and pinned Chromium, workers 1, retries 0; focused formatting and diff
  checks passed. This ledger commit is later than the tested source.

- Frozen source `745008ccb215fec2f7b56922ac43b7e2261166d7`, clean at start.
  Native host Mac16,12, Apple M4, arm64, translation status 0, macOS 27.0.1
  (26A434), Node v24.13.0, pnpm 12.8.2, Playwright 1.61.0, pinned WebKit
  26.5 revision 2311. Started 2026-11-05 18:03 UTC via the three-spec
  guarded command in the continuation request, workers 1, retries 0. First run
  in progress: all 15 modifier cases passed; empty form target router and native
  oracle timed out, as did empty submitter target router. These are not a pass.
  The target suite hard-codes Chromium popup behavior; diagnosis is incomplete.
- Implementation CI job 127258777637 logs were inspected: 221 Chromium and
  10 packed-consumer tests passed. The earlier unknown counts reflect the
  prior session's access limitation, not this later inspection.
- PR #4599's misleading sentence was replaced with neutral related-tracker
  wording, and #1492 was reopened; both results were read back. Draft PR #4600
  was created for this branch. Final-head checks and production merge remain
  pending.

- Starting main: `9b68b8f772ad30b0d28b3e3924c8ed4739ba85ce`. Refreshed
  public PR list: zero open. Main health 41978920099, router 41978920267,
  Wind 41978920264, and binary/scaffold/showcase 41978920344 succeeded.
  The adopted-owner receipts below and PR #4599 identities remain historical.
- Branch: `base/router-submit-modifiers`. Clean frozen implementation:
  `f36dd2ffca6925fe0c351bfe6cb891c65525a763`. This evidence commit is later.
  Manager owns integration, verification, and CI triage. One focused PR was
  requested; no planning issue/epic or publication work.
- L4 trusted-input native oracle: identical fixtures, router module stubbed.
  Control-click on Debian Chromium targets the actual span/SVG child; a
  button-only coordinate supplies the direct control. Context pages, trusted
  click/submit details, document identity, lifecycle, errors, wire requests and
  payloads are observed before activation. The native modified forms open one
  new tab and leave the original document/URL intact. GET query and explicit
  URL-encoded POST body are `q=hello+world&go=search`, exactly one request.
- Unchanged runtime: first complete matrix **4 failed / 11 passed**. Four
  descendant rows (span, SVG, external form-associated button, POST) incorrectly
  fetched in the original document, with normal SPA lifecycle, instead of the
  native new-tab document navigation. Strengthening canceled/invalid controls
  to direct-button clicks exposed both stale latches: final unchanged-source
  matrix **6 failed / 9 passed**, retries 0. Later genuine
  `requestSubmit(button)`, without another click, navigated natively instead
  of using the SPA. Blocked actions themselves made zero requests.
- Repair captures `composedPath()` during click dispatch, matches the actual
  submitter, checks late cancellation, and consumes state once. A task timer
  expires unused state after trusted activation; no microtask cleanup or
  timestamp heuristic. Links check their own event modifiers independently.
  No realm-sensitive restriction, form-owner, encoding, sourceElement,
  FormData/hook, target or history refactor.
- Host: Linux Debian 13, x86_64, Node 24.19.0, pnpm 12.8.2,
  Playwright 1.61.0, system Chromium 154.0.8037.57. The pinned Chromium
  149.0.7827.55 download was denied by network policy. A temporary local config
  selected system Chromium; no browser override was committed. This browser
  omits native popup initial requests from Playwright context request events.
  The small fixture-server recorder verifies wire method/body/query and
  `Sec-Fetch-Dest` (document versus empty/fetch), cross-checking emitted browser
  requests. The original page and every new page remain observed.
- First harness construction failed on popup request observation and a popup
  init-script `crypto.randomUUID` absence; a diagnostic selector also selected
  zero tests. Those are harness failures, not runtime verdicts. They were
  corrected before the decisive unchanged-source matrix; original logs remain
  in session-local work storage. All behavioral runs used retries 0.
- Post-fix matrix: **15/15**, first attempt. Clean frozen-source related L4
  verification: **140/140**, no skips, retries 0, heavy-guard PASS, 76 seconds.
  Suites: form-modifier 15, form-submit 63, link-intent 28, form-encoding 7,
  form-metadata 14, form-owner 13. Includes canceled/invalid then genuine
  requestSubmit and trusted keyboard activation, HTML/SVG/area links, direct
  submit button/input and ordinary child SPA controls.
- Affected router units **155/155**, runtime typecheck, focused formatter,
  syntax and diff checks passed. The commit hook initially failed because the
  host default pnpm is 11.25.0; command-local Corepack pnpm 12.8.2 satisfied
  the existing hook without bypassing it.
- Independent read-only reviewer (inherited model/effort; exact settings
  unknown) found no actionable issues. A proposed invalid-listener microtask
  requestSubmit case made no native submission: the native reentrancy guard
  suppresses it while validation is firing. Its failed native-oracle probe
  was preserved and the finding withdrawn; no speculative repair was added.
- Native M4 WebKit: **pending, zero executed**; no Mac available. The exact
  required three-spec command selects **106 cases** (15 modifier, 63 submit,
  28 link). Selection is not execution. Qualify the frozen implementation
  above, record native host/architecture/OS/tool/browser versions, command,
  first results/skips/retries, and use the existing heavy guard. No Neo rerun.

  After rebuilding the browser prerequisites once on the frozen source, run:

  ```sh
  bash "$HOME/.codex/scripts/heavy-guard.sh" -- env ZFB_BROWSER_ENGINE=webkit \
    pnpm exec playwright test --config tests/router-chromium/playwright.config.mjs \
    form-modifier.chromium.spec.mjs form-submit.chromium.spec.mjs \
    link-intent.chromium.spec.mjs --project=webkit --workers=1 --retries=0
  ```

- Implementation router CI [42031487327](https://github.com/Takazudo/zudo-front-builder/actions/runs/42031487327)
  succeeded on `f36dd2ffca6925fe0c351bfe6cb891c65525a763` (2m47s), including
  the existing full browser/packed-consumer job. Artifact digest:
  `sha256:41b339b1162c8987f3f6b1b294680c437274e237c7c27ae42f415f48086875e5`.
  Public status/artifact identity were visible; signed-in logs were unavailable,
  so CI's exact counts and retry history are unknown. Wind
  [42031487330](https://github.com/Takazudo/zudo-front-builder/actions/runs/42031487330)
  also succeeded at that implementation SHA. No broad local Rust or duplicate
  full browser run.
  Required PR checks and approvals are unverified; source CI is not final
  evidence-ledger-head verification.
- GitHub metadata: rendered #1492 timeline event **35210077709** explicitly
  identifies PR #4599 as its closer (event created 2026-11-05T06:12:54Z).
  No superseding owner retirement decision appeared in the inspected timeline;
  the latest owner-facing comment still lists outstanding human acceptance.
  The misleading sentence-only replacement was prepared. Both PR-body edit
  and issue reopen failed at `https://api.github.com/graphql`: Forbidden.
  Both metadata objects remain unchanged; no replacement tracker was created.
- Draft PR creation failed at that same GraphQL restriction. The branch is
  pushed, but no task PR exists. Conditional merge is held for PR/check access,
  final-head policy/approval verification, and actual native M4 qualification.
  Inspected triggers: main deploys the production showcase; docs deployment
  requires docs/workflow paths; drafts suppress existing preview uploads.
  This diff contains no docs, dependency/version/lockfile, workflow, public API,
  native size-contract or release changes. Related trackers outside runtime
  delivery remain #1492, #4276, #4576, #4123, #4366, #4304, #3357 and #4490.

## Adopted submitter form owner — 2026-11-05 UTC

This checkpoint records the client-router form-owner repair. The sections below
stay historical. No Neo rerun. This does not close #1492, #4276, #4576, #4123,
#4366, #4304, #3357, or #4490.

- Starting main: `07230175e17d096e2cf4794e54970d89ae266e5c` (#4598). Health
  41906139729, router-chromium 41906139731, and binary/scaffold/showcase
  41906139804 succeeded. Zero open PRs.
- Branch: `base/router-form-owner`. Implementation
  `0dfd6784cc643ba678459b1b2a4ad4ca3f44e21f`. This ledger commit is later.
  Browser evidence below is that implementation SHA, clean tree, retries 0.
- Host for every local run: Mac16,12, Apple M4, arm64,
  `sysctl.proc_translated=0`, macOS 27.0.1 (26A434), Node v24.13.0, pnpm 12.8.2,
  Playwright 1.61.0, Chromium 149.0.7827.55 rev 1228, WebKit 26.5 rev 2311.
- Defect, unchanged runtime at the starting main SHA, new
  `form-owner.chromium.spec.mjs`, heavy-guard **FAIL** exit 1, 7s, min memory
  7421 MB, first attempt: **6 failed / 7 passed** of 13. Adopted external
  button: native `application/x-www-form-urlencoded`
  `owned=caf%C3%A9%26%3D%E9%9B%AA&go=send`, router `multipart/form-data` with
  those same fields. Adopted external `input[type=submit]`: native
  `owned=yes&go=send`, router multipart. Adopted submitter in a multipart
  decoy: native `go=send&owned=yes`, router multipart. Adopted submitter in a
  URL-encoded decoy with a file: native multipart including `hello-file`,
  router `application/x-www-form-urlencoded;charset=UTF-8`
  `go=send&owned=yes&note=note.txt`. Foreign `form="missing"` inside a
  URL-encoded form, via `navigate()`: router `application/x-www-form-urlencoded`
  instead of multipart. The preparation-hook case failed on that same
  multipart fallback; its source-identity assertion had already passed.
  Passing controls: four `formenctype` rows (the attribute is read from the
  submitter, so the wrong owner did not change the result), same-realm
  external, adopted in-owner, and same-realm null owner.
- Repair: `defaultLoader` still brand-checks a direct form first. A
  form-associated element uses the `.form` getter on its own prototype,
  including null. Elements with no form-owner API still use the ancestor
  form. `sourceElement` is unchanged. Encoding stays after preparation hooks.
- happy-dom shares prototypes across its iframe window and prefers its
  internal form node over the `form` attribute, so it cannot represent this
  adoption bug. New unit coverage is select, textarea, and the ancestor
  fallback. `router.test.ts` on the implementation SHA: **150/150**.
  `pnpm -C packages/zfb-runtime typecheck` passed on that same TypeScript.
- Post-fix Chromium, heavy-guard **PASS** exit 0, 5s, min memory 7134 MB,
  first attempt: **13/13**.
- M4 WebKit, heavy-guard **PASS** exit 0, 15s, min memory 6866 MB, first
  attempt: **20/20** (`form-encoding` 7, `form-owner` 13). No skipped tests.
  Owner decision for this form work: the M4 run is the macOS qualification.
  No Neo-specific rerun.
- Not shipped: no package version, lock, runtime API, native size contract,
  or release procedure change. Published 5.2.0 predates this fix.

## Packed-consumer realpath containment — 2026-11-05 UTC

This checkpoint records the packed-consumer temp-path repair. The form-metadata
section below stays historical, including its pre-merge wording and the macOS
`/var` versus `/private/var` failure. #4597's M4 WebKit qualification stays
completed. No Neo rerun.

- Starting main: `99ca3cf7d24a3e314f92c24d1317546915863485` (#4597). Health
  41880690630, router-chromium 41880690637, Wind 41880690633, and
  binary/scaffold/showcase 41880690646 succeeded. Zero open PRs. The eight
  open issues are unchanged.
- Branch: `base/packed-consumer-realpaths`. Implementation
  `16339c5c396a1b31628dff8263a28a630abc6cd3`. An independent reviewer traced
  alias acceptance against package, entry, and `node_modules` escapes and
  reported no issues. This ledger commit is later than that implementation
  SHA. The packed evidence `source` field is the implementation SHA.
- Repair: the trust anchor is the physical path of the fresh isolated
  directory. The real `node_modules` directory must remain inside it, and the
  real SDK and runtime entries must remain inside that installation. An
  escaped `node_modules` realpath is rejected rather than adopted as a new
  root. Containment is component-aware. The dist-entry shape checks stay
  `/dist/runtime.js` and `/dist/client-router/index.js`.
- Root unit file `scripts/__tests__/native-regions-packed-paths.test.mjs`,
  `pnpm exec vp test run` on that file, first post-fix attempt: **8/8**
  (334 ms). `node --check` and `vp fmt --check` passed on the helper, the
  checker, and the test. Physical temp root: accepted, and the old lexical
  prefix also held. Symlinked ancestor: accepted, while the old lexical
  prefix failed and the canonical prefix held. In-tree `.store` package
  symlink: accepted. `node_modules-other`: rejected, where the old prefix
  matched. Package symlink to an outside directory: rejected. Entry symlink
  to `tests/router-chromium/fixture/island-real.js`: rejected. Entire
  `node_modules` symlink to an outside directory: rejected. Missing SDK
  entry, unexpected SDK shape, unexpected runtime shape, and a relative
  isolated root: failed closed. The physical, alias, in-tree, package-escape,
  and `node_modules`-escape cases used a real Node `import.meta.resolve`.
- M4 packed consumer, normal macOS temp, heavy-guard **PASS** exit 0, 17 s,
  min memory 8397 MB, first attempt, retries 0. Playwright project chromium,
  **10/10** in 8.2 s. `ZFB_BROWSER_ENGINE` and `ZFB_REGION_PLAYWRIGHT_CONFIG`
  were unset. `TMPDIR` was `/var/folders/rh/yslrhq516hx43rh8ln83vmfh0000gn/T`,
  canonical `/private/var/folders/rh/yslrhq516hx43rh8ln83vmfh0000gn/T`
  (`alias: true`). No non-symlink `TMPDIR` workaround.
- Host: Mac16,12, Apple M4, arm64, `sysctl.proc_translated=0`, macOS 27.0.1
  (26A434), Node v24.13.0, pnpm 12.8.2, Playwright 1.61.0. Raw isolated path
  `/var/folders/rh/yslrhq516hx43rh8ln83vmfh0000gn/T/zfb-native-regions-packed-ZY4F1I`.
  Canonical root
  `/private/var/folders/rh/yslrhq516hx43rh8ln83vmfh0000gn/T/zfb-native-regions-packed-ZY4F1I`.
  SDK and runtime entries resolved under that root's `node_modules`. Tarball
  SHA-256: `takazudo-zfb-5.2.0.tgz`
  `96cabbedc2fab9d42e4cb9dedf7690b420effa2f55ebe307ddf5f832120b3b6f`,
  `takazudo-zfb-runtime-5.2.0.tgz`
  `e2e41414afc042a8af3631c8ef2f00c24b950acf3147f9d20c17093bd18a9def`,
  `takazudo-zfb-slugify-5.2.0.tgz`
  `618690f20631aeeb73021b4888f6c7990e2b1254ddc84cc7a3cf87b70bb102e5`.
- Outstanding at this commit: required PR CI on the branch head, then main
  integration. No package version, lock, runtime API, form behavior, native
  size contract, or release procedure changes. This does not close #1492,
  #4276, #4576, #4123, #4366, #4304, #3357, or #4490.

## Client-router form metadata shadowing — 2026-11-05 UTC

This checkpoint supersedes the operational status of the dated entries below.
Their receipts stay historical evidence. It does not record a main merge.

- Starting main: `3048d35cdcb02b184d602d85a66efd2b03663dc7` (#4596). The health,
  router-chromium, Wind and binary-smoke push runs on it all succeeded
  (41848528224, 41848528209, 41848527916, 41848528084). Zero open PRs. The eight
  open issues are unchanged: #1492, #4276, #4576, #4123, #4366, #4304, #3357, #4490.
- Branch: `base/router-form-metadata`, draft PR to main. Implemented, reviewed by
  an independent reviewer (two low findings fixed), not integrated into main.
- Defect: controls named or id'd `getAttribute`, `tagName` or `dataset` shadow those
  form properties. `handleSubmit` now classifies the target with a brand check through
  `Element.prototype` getters. That check is realm-independent, so adopted
  foreign-realm forms still route. It reads `data-zfb-reload`, `action`, `target` and
  `method` through `contentAttribute()`. `defaultLoader` uses the same brand check, so
  a foreign-realm form with a `closest` control no longer throws after `preventDefault()`.
- Unmodified-runtime Chromium (`form-metadata`, 12 cases at the time, retries 0):
  **10 failed / 2 passed**. `getAttribute` threw (`form.getAttribute is not a
  function`) and fell back to a native load. `tagName` and the RadioNodeList variant
  went native. `data-zfb-reload` with a `dataset` collision was routed. The two
  controls passed. The later `closest` case failed at `619b814a` with
  `source?.closest is not a function`.
- Local, clean head `58931def9d48f14146917479ac8f00ec337c82e0`, Apple M4 `Mac16,12`
  arm64 (not translated), macOS 27.0.1 (26A434), Node 24.13.0, pnpm 12.8.2,
  Playwright 1.61.0 (Chromium rev 1228, WebKit 26.5 rev 2311). Every result below is
  first attempt with retries 0. `pnpm -C packages/zfb-runtime test` **450/450**
  (the three new branch tests fail on unmodified source). Typecheck and
  `format:check:ts` passed. `pnpm test:router-chromium` **193/193** (179 + 14 new).
- Packed consumer: the first run at `619b814a` failed before testing. Its assertion
  compared the `/var` tmpdir with the `/private/var` realpath (a macOS symlink
  harness issue, not a product failure). With `TMPDIR` set to a non-symlinked
  directory it passed **10/10**, at both `619b814a` and `58931def`.
- WebKit on that M4 (`form-entry-encoding` + `form-metadata`, workers 1): **25/25**.
  Owner decision (2026-11-05): this native macOS arm64 WebKit run on the M4
  completes the macOS delta qualification for this form fix. No Neo-specific rerun is
  required. The identity above stays M4, not Neo, and earlier Neo receipts are unchanged.
- Not shipped: published ZFB 5.2.0 predates #4592, #4596 and this fix.

## MacBook Neo native qualification at main `54054042` — 2026-11-04 UTC

This checkpoint supersedes the operational status of the dated entries below.
Their original results, failures, retries and tuples remain historical evidence.

- Qualification SHA: main `54054042e5706cfa0d8c365913a8372fb7e864a1` (#4592),
  clean, with no repair needed. Machine: MacBook Neo `Mac17,5`, Apple A18 Pro, 8 GB,
  macOS 27.2 (26B123), native arm64 (not translated). Node v24.14.0 arm64,
  pnpm 12.8.2, frozen install, `pnpm-lock.yaml` `5cce7d76…277d`, Playwright
  1.61.0 with WebKit 26.5 and Chromium 149.0.7827.55. Every run used the heavy
  guard, ran first attempt, and had zero retries and zero skips.
- Current-source WebKit, **20/20**:
  - `pnpm test:webkit-back` **4/4**: three back-navigation cases and the native
    History-quota rejection/recovery. The runtime was rebuilt in the same chained
    command. Case 3 dispatches a synthetic `hasUAVisualTransition` popstate; it is
    not a physical iPhone swipe.
  - `pnpm test:webkit-token` (canonical builds, registry off): `sdk-persist`
    **1/1** and `r-a08-mismatch` **1/1**.
  - #4592 `svg-link` + `form-encoding` in WebKit: **14/14** (7 + 7).
- Published Darwin arm64 consumer, tuple ZFB/runtime/md-wasm 5.2.0, doc 9.1.2,
  SG 0.8.1, ZDTP 0.9.0, fixture lock `334a2ff9…8f92`: **PASS** (188 s). The
  installed carrier is `@takazudo/zfb-darwin-arm64@5.2.0`, Mach-O arm64, SHA-256
  `be76d1f10dbbca442947d9a9331ae38c889a298215685f3d7a6220788ded67ef`. The positive
  case passed all identity and seven browser steps; both negative controls failed
  for their documented reasons. Published 5.2.0 predates #4592
  ([receipt](https://github.com/Takazudo/zudo-front-builder/issues/4276#issuecomment-6627369064)).
- Darwin size: a fresh native collection at the qualification SHA (one guarded
  1.99.0 `cargo build --locked -p zfb`, debug info off by the 20 GiB rule). Both
  eight-fixture matrices were measured twice, and `check-budget.mjs` exited 0 on
  the first attempt. The release-bump stage (receipt `a0dd8e36…a934`) was
  independently reviewed and applied with owner authorization; see
  [RESULT.md](v3-island-size/stage-evidence/darwin-release-5.2.0/RESULT.md).
  All 16 totals are unchanged; only provenance moved.
- Deployed docs at `54054042` (Production Deploy 41788226142): the agent-driven
  headed Chromium walkthrough passed **13/13** with 0 console errors. Mobile was
  viewport emulation. The owner's own browser result is **NOT RUN**, so #4276 and
  #4576 stay open for it.
- Unchanged holds: #4123 and #4366 (native Intel), #4304 (genuine
  scheduled-watch recovery), #3357 and #4490 (upstream/owner triggers).

## Client-router SVG base resolution and form encoding — 2026-11-04 UTC

This checkpoint supersedes the operational status of the dated entries below.
Their original results, failures, retries, and qualification tuples stay historical
evidence. Nothing here verifies a later head, and it does not record a main merge.

- Refreshed starting main: `9a7b91bc72ffc806010e5a17375ecf8b8c8e71e5` (#4591).
  Zero open PRs. The eight open issues are unchanged: #1492, #4276, #4304,
  #4576, #4123, #4366, #3357, #4490. No package version, lock, size contract,
  fingerprint policy, CI schedule, or release procedure changed.
- Development branch: `base/router-svg-form-encoding`. Status at this writing:
  **implemented** and **reviewed** locally. The full local router suite and the
  packed consumer check below ran on this working tree. Required CI and main
  integration are not done. This entry does not name a commit SHA, because it
  is written before that commit. A later note that only repeats CI numbers is
  a different checkout.
- SVG `<a>`: one absolute URL is resolved from `href.baseVal` (the browser
  reflects `xlink:href` when `href` is absent, and `href` wins when both are
  present) against the element base URI, then used for both the origin check
  and `navigate()`. `navigate(relative)` still resolves against `location.href`.
- Form encoding: a present submitter `formenctype` wins, including when empty.
  Otherwise the form `enctype` content attribute. Neither attribute keeps
  multipart `FormData`. Explicit empty or unrecognized keywords select
  URL-encoded. Keywords are ASCII case-insensitive and are not trimmed. POST
  `text/plain` returns before `FormData` and `preventDefault()`. GET ignores
  encoding. Imperative `navigate(url, { formData })` has no new native fallback and
  no new public option.
- Unmodified Chromium reproduction, runtime built from that main SHA before the
  router edit, Node 24.13.1 / pnpm 12.8.2 / Playwright 1.61 / Chromium
  149.0.7827.55, heavy-guard **FAIL** exit 1, 233s, retries 0: **9 failed / 5
  passed**. Failures: base-relative SVG navigated to `/source/next.html`;
  cross-origin base did the same; fragment-only under another base did not
  leave the source document; relative `xlink:href` navigated to
  `/source/from-xlink.html`; multipart form plus URL-encoded submitter sent
  multipart; URL-encoded form plus multipart submitter sent URL-encoded; both
  `text/plain` POST paths stayed SPA; `text/plain` overridden to URL-encoded
  stayed multipart. The five passes on that same run were no-base SVG,
  path-absolute SVG, `href` over `xlink:href`, absent-encoding multipart, and
  GET `text/plain`.
- Repaired focused Chromium of those same 14 tests: heavy-guard **PASS** exit
  0, 141s, **14/14**, retries 0. `pnpm -C packages/zfb-runtime test` passed
  **443/443** before the final formatter pass. After `vp fmt`, the same router
  unit file passed **139/139** and `pnpm -C packages/zfb-runtime typecheck`
  passed. Later edits did not change that unit file.
- Full `pnpm test:router-chromium` on the formatted runtime, before two
  stronger request assertions: heavy-guard **PASS** exit 0, 215s, min memory
  9264 MB, **168/168**, retries 0. The same Chromium build.
- Independent Codex review of the uncommitted diff found two defects: the
  multipart-file browser test did not prove router ownership, and the docs
  called `navigate({ formData })` instead of `navigate(url, { formData })`.
  Both were fixed. A second pass reported no findings. The reviewer did not
  execute tests. Empty SVG `href` remains an untested early return.
- After those assertion fixes, the multipart-file and absent-encoding tests
  were re-run: heavy-guard **PASS** exit 0, 138s, **2/2**, retries 0. Each now
  requires `resourceType === "fetch"` and a retained document sentinel. The
  other 166 tests were not repeated after that assertion-only edit.
- Packed consumer `tests/router-chromium/check-native-regions-packed.mjs`:
  heavy-guard **PASS** exit 0, 14s, **10/10**. Disk had 320 GB free. The
  evidence JSON's `source` field is still
  `9a7b91bc72ffc806010e5a17375ecf8b8c8e71e5` because the tree was dirty. The
  packed runtime tarball `takazudo-zfb-runtime-5.2.0.tgz` sha256
  `ff5e46b066d165899674c79aa658b44ffc1e09336ee9d8646e37795ac30c5763` contains
  `selectedFormEncoding` and `new URL(rawHref, linkEl.baseURI)`. That tarball
  is the checked artifact. It is not identified by the parent SHA alone.
- `pnpm docs:check` passed: 2 collections and `tsc --noEmit`, no errors. That
  command does not build the site. The docs-site build remains the CI Docs gate.

## Published consumer adoption checkpoint — 2026-11-03 UTC

This checkpoint supersedes the operational status of the dated entries below;
their original results, failures and holds remain historical evidence.

- Integrated router delivery: #4589 source
  `6d4bb6625d8f0c4ca024104326a8789df79729d9`, main merge
  `1ff3adb4ade5ad70545feccf4e0a2bd2576c4e64`. The owner subsequently released
  ZFB **5.2.0** from `0f17c8e947548b4c4a82ebce112bd5f026e1b21b` on Mac.
- The downstream adoption lap is integrated: zudo-doc #4694 adopted ZFB 5.2.0;
  release #4695 merged at `4b5b54e3e0f383cfbf1e7b6038c6b65ee7c5f9d2` and
  published doc/history/create **9.1.2** through the normal package lanes.
  zudo-sg #1041 adopted ZFB 5.2.0 and #1042 adopted doc/history 9.1.2, merged at
  `d8b9e2ac3ca3664f0c2c39fa0df68073e538d20d`. Current-head CI and automatic
  production deployments passed for those deliveries. They do not supply native
  ZFB acceptance or close independent trackers.
- Development branch: `base/dogfood-zfb-5-2-doc-9-1-2`, from main
  `0f17c8e947548b4c4a82ebce112bd5f026e1b21b`. Independently reviewed dependency
  source: `90ce206fb06c76478ff233ed1532c74001cd36a1`. Ledger commits, PR test refs
  and any later main merge are separate identities. This candidate is not yet
  integrated and does not change or publish a ZFB package version.
- Docs host: ZFB/adapter/runtime/md-wasm **5.2.0**, doc/history **9.1.2**.
  The published-preset fixture is a separate changed tuple: ZFB/runtime/md-wasm
  **5.2.0**, doc **9.1.2**, SG **0.8.1**, ZDTP **0.9.0**. Its lock was rebuilt
  outside the workspace; registry integrity checks and both generator checks
  pass, with unchanged generated output (one story, 102 tokens).
- Published template/tsconfig comparisons preserve the host adaptations and
  original unknown route provenance. Local docs type/collection checks and
  63 carrier/gate/fingerprint unit tests pass. The dependency fingerprint remains
  `47de8f7592f1fb1c463ad9294ea65bbc1f315d50a74e87ab86a129d86eda89b2`;
  live size contracts, build configuration and fingerprint policy are unchanged.
- Local Node 24.19.0 / pnpm 12.8.2 / Chromium 149.0.7827.55 verification passed
  first attempt under the heavy guard (836 seconds): root and genuine subpath
  builds each emit 783 pages; root HTML and 106 island-marker pairs pass; both
  route/anchor and Markdown-link audits pass; Chromium **178/178** at each base
  (**356 total**, zero retries). Doc history generation is explicitly skipped
  via the existing `DOC_HISTORY_SKIP_POSTBUILD=1` option, not claimed tested.
- Linux x64 installed-carrier fixture on checkout
  `e7f0299002e1d7cd3ac95bc0d326ef783707d67c` passed: positive hydration, signals,
  theme, SPA cleanup and history remount; both negative controls failed for the
  required identity/hydration reasons. Carrier 5.2.0 SHA-256:
  `6b0d182ed98d302f2e9aaacdcf8b2f759ca93b4599c10d8ef6eacd65addedde1`;
  fixture lock SHA-256:
  `334a2ff9f2db73f397f92efa048f78a111c20f30fb32659574bc49acc47a8f92`.
- Push CI on that exact ledger head passed first attempt: router **154/154**
  plus packed **10/10** (41697463987), Wind **28/28** (41697463991), React browser
  **22/22** (41697463985). Later ledger-only commits are distinct from that tested
  checkout. Workspace-compiler fixture and broad required Linux/Docs checks
  remain pending on a PR; these push workflows do not replace them.
- Delivery hold: automatic approval review rejected creation of the public draft
  PR from this branch to main, citing missing explicit authorization for this
  specific public PR. A read-only check confirmed no PR was created. No retry
  or alternate publication route was used. The reviewed branch is pushed;
  required PR CI and the separate docs/showcase production boundary remain.
- Delivery follow-through: after additional user continuation evidence, the single
  authorized retry created draft **#4591**. Updating its body with completed
  verification was separately denied by automatic approval review as a public
  metadata update; that action was not retried. The PR remains draft.
- First PR CI on head `ea93ec7e6993703a320e7134d3248f7488864363` tested synthetic
  merge `dfd8b05cb8732f8f9adee282198dafc37cc497d4`, whose tree matches the head.
  Docs run **41699806063** failed root Chromium **177 passed / 1 failed**; Docs
  gate consequently failed and subpath execution was skipped. Artifact
  **13264200177**, SHA-256
  `37d0b2084636f56cdfdec9c9abe1b96254b12de059f5038f4c73caff6db372fc`, preserves
  the trace: the mobile Claude navigation helper inspected the Open button before
  `after-swap`, skipped its click, then asserted a drawer that navigation closed.
  URL/visible-heading checks had not established navigation completion.
- The test now records `zfb:page-load` readiness, clears it before preparation,
  and waits for the current path to settle before checking navigation controls.
  All original assertions and native clicks remain. No timeout increase, sleep,
  retry or product workaround was added. Focused genuine-subpath desktop/mobile
  checks passed **10/10** (five repetitions each, heavy guard PASS in 51 seconds).
  The original CI failure remains distinct; replacement-head full CI is pending.
- New-tuple native acceptance is **NOT RUN**. The old arm64 installed-carrier
  receipt at `5de7215af` belongs only to its ZFB 5.1.5 / doc 9.1.0 / SG 0.8.0
  tuple. Cloud Chromium is not a replacement for that native receipt or the
  owner's browser acceptance. Any future ZFB release is owned by the user's Mac
  workflow; this session performs no release, tag, publication or manual deploy.

Open-issue refresh found no new decision-free implementation in the existing
owner/native/watch/upstream trackers. Keep #1492, #4276, #4576, #4123 and #4366
under their original criteria. #4490 still retains the per-edge external-ID
ambiguity trigger; no dependency upgrade or speculative compatibility policy is
introduced. #3357 remains deferred: the official [cf migration documentation](https://developers.cloudflare.com/cf/wrangler/migrate/)
still identifies the CLI as beta on this refresh. zudo-doc's remaining trackers
require actual account acceptance; zudo-sg #963 retains the supported
module-identity remedy criterion, which clean-install tests do not satisfy.

#4304 remains open. The latest scheduled watch is still **41277672402**; its
workflow success does not mean the watch is green. Artifact **13072061388**
(SHA-256 `bce83d3f5e7e58ef24dfb564faa06923eb376d466c66f902295d0a7bbcd23fde`)
was independently downloaded: strict-main **25 runs / 75 records**, 50 ok,
25 expected failures, **zero unexpected failures**; doc generations 7.1.0 ×13,
8.1.1 ×6, 9.0.0 ×2 and 9.1.0 ×4 explain the red identity verdict. Maximum pre-UP
is 118 ms against 12,000 ms. An initial signed-URL download returned HTTP 403;
the connector download succeeded with matching digest. No dispatch, narrowing,
drift allowance or closure was performed. The next genuine rolling-window
result owns recovery, and this docs update adds another intentional identity
transition that must remain visible.

## Client-router native intent and history checkpoint — 2026-11-03 UTC

PRs #4587 and #4588 are already integrated. The refreshed starting main is
`41e7840febd802d52a99f0db1a1223a7b6ee725b` (#4588); neither implementation is
reopened. The dated receipts below remain historical evidence.

- Development branch: `base/client-router-native-intent-history`. Reviewed runtime
  source: `7684db31dbdd6e09cf855947ba69b53178770405`; subsequent ledger commits
  are separate from that source identity. Draft delivery PR: #4589. No new main
  integration has occurred; PR merge refs are CI inputs, not production commits.
- Implemented: effective anchor/area base targets and case-insensitive self;
  single document-base form action resolution with empty-action/current-URL and
  shadowed-control behavior preserved; null-state same-document traversal cancels
  obsolete fetch/preparation/queued swaps while preserving reentrant navigation.
  Completed fragment navigation relinquishes ownership before synchronous native
  hash events. Independent source reviews completed, including the follow-up fix.
- Local Node 24.19.0 / pnpm 12.8.2: frozen install, runtime tests **423/423**,
  runtime typecheck, changed-file formatting and diff checks pass. Final real
  Chromium **154/154** and isolated packed-consumer **10/10** pass under the
  heavy guard on the reviewed source. Push CI on ledger head `dda5dda40` passed
  router **154/154** plus packed **10/10** in run **41605405641** (artifact
  **13217080449**) and Wind **28/28** in run **41605405633**, without test retries.
  Required PR CI remains pending; no main integration is claimed.
- Original-source regression evidence: task-a L2 **14 failed / 104 passed**;
  real Chromium relative/cross-origin GET/POST action cases **4 failed**. History
  Chromium JSON fallback, HTML cancellation and queued VT cases **3 failed**;
  managed traversal control passed. Task-b focused L2 reproduced five failures.
- Preserve first failures: the native empty-link oracle initially expected self
  (**12 passed / 2 failed**); actual popup observation established base fallback
  for both anchors and areas (**2/2**). Six initial L2 expectations incorrectly
  stripped fetch-input fragments; corrected separately from network assertions.
  The happy-dom null-popstate seam needed the existing explicit-state idiom.
  Review found and fixed premature dialog action parsing. First integrated
  Chromium run was **153 passed / 1 failed**: completed managed-fragment navigation
  emitted a spurious abort. A new L2 regression reproduced it before the fix;
  focused history tests then passed **25/25**, with existing browser assertions
  unchanged. The first commit hook used host pnpm 11; process-local pnpm 12.8.2
  corrected that toolchain failure without bypassing hooks.
- Dependency fingerprint remains
  `47de8f7592f1fb1c463ad9294ea65bbc1f315d50a74e87ab86a129d86eda89b2`.
  Existing island-size enforcement tests pass **127/127**; dependency graphs,
  versions, locks, build configuration and live size contracts are unchanged.
  Current-source native measurements and broad Rust verification still belong to
  the existing Health CI lane; those are not inferred from the fingerprint.
- Initial delivery hold: automatic approval review denied draft PR creation, including a
  second attempt after read-only confirmation that the repository is public.
  Its stated remaining reason was insufficient explicit user authorization for
  the external PR commitment. No alternate publication route was attempted.
  After the user explicitly approved that exact draft PR action, the single
  authorized retry created #4589. Main's eleven required contexts plus task-specific router Chromium, packed
  consumer and island enforcement remain merge gates. Main merge also triggers
  automatic production showcase deployment; no manual deployment, release, tag
  or package publication is performed.

The implementation manager owns remaining CI diagnosis and final integration.
Keep #1492, #4276, #4304, #4576, #4123, #4366, #3357 and #4490 under their
existing independent owner/native/watch/upstream criteria. This work supplies no
owner-browser confirmation, native Intel execution, upstream trigger or genuine
scheduled-watch recovery; the strict rolling watch and historical receipts remain.

## Native admission follow-through — 2026-11-03 JST

PR #4587 now carries the owner-approved paired dependency-graph re-pin for frozen
candidate `d04ef461b3e9c54cedd1be8ef46bee219c716690`. This section supersedes the
admission hold and #4458 disposition in the November 2 checkpoint below.

- Native Darwin arm64: Rust/Cargo 1.99.0, Node 24.14.0, zlib 1.3.1-e00f703,
  pnpm 12.8.2, esbuild 0.25.12. Both eight-case modes retain all raw and
  gzipNormalized totals with zero allowances. Source remained clean.
- Original collection passed build/measurements but its receipt was refused because
  the handoff recorded a multiline shell argument. The validator was unchanged;
  a corrected external driver invokes a retained provenance script and records real
  argv. Recollection passed build/workspace/packed under the heavy guard in
  33/30/23 seconds. Both collections and the original rejection remain retained.
- Corrected receipt SHA-256:
  `9a7e8edf0aeca85c470611e979fc3c5f7e34cb6477b6f91dd93e1c48f433bd54`.
  Bundle digest: `8762ed5c9e6ed585bfc98eddfbc9e2f695f27b03c3fbdf551e85891249b4d4eb`.
  Linux evidence: health run 41460157485, job 125327048902, artifact 13150621471,
  measured merge source `9a7a1c08f269a0f62d8c3b422e4fb901315a20e3`.
- Independent review recomputed normalization, checked command and binary identity,
  and verified the paired stage. The owner approved applying manifest SHA-256
  `51d9ebf0e4c1c87d26524f14322ba4d3f86beaf143dc39ae93788dfbe498ba60`.
  Application succeeded; the enforced Darwin checker now passes all eight cases
  in both modes. The original fingerprint-only exit 1 remains historical evidence.
  Final-head CI is tracked on the PR; pre-application CI does not qualify this update.
- #4458 is closed after direct inspection of scheduled exam 41168210490's JUnit:
  both scanner cases passed without retries. #4304 remains red from mixed doc
  identities in watch 41277672402, with no unexpected failures or timing breach.
  Intel and owner-browser obligations remain open. Main merge/release is not performed.

## Current checkpoint — 2026-11-02 UTC

Integration/CI triage owner: the project integration manager. Read-only refresh of GitHub
issues (including comments), releases, open PRs and Actions metadata on 2026-11-02.
This checkpoint supersedes the operational status in every historical snapshot below;
the original receipts, failures, decisions and dated holds remain preserved there.

### Production, candidate and qualification identities

| Lane | Identity | State and evidence |
| --- | --- | --- |
| Production `main` | `5de7215afc89fae8d5881983b8e8ade9a14ca093` (#4586) | Docs host uses zudo-doc/history-server **9.1.0**. Latest ZFB release is [v5.1.5](https://github.com/Takazudo/zudo-front-builder/releases/tag/v5.1.5), published 2026-10-26. Initial refresh found **0 open PRs** before this candidate. |
| Development candidate | `base/docs-9-1-1-followthrough`, starting at the same `5de7215af` | zudo-doc/history-server **9.1.1** adoption is candidate work. It is not production, a released ZFB version, or new owner-browser acceptance. The manager owns integration and candidate verification. |
| Published-preset fixture | ZFB/runtime/md-wasm **5.1.5**, doc **9.1.0**, SG **0.8.0**, ZDTP **0.9.0** | Keep this coherent committed tuple unchanged in the 9.1.1 docs-host work. [Native installed-carrier acceptance](https://github.com/Takazudo/zudo-front-builder/issues/4276#issuecomment-6522863052) passed first attempt on arm64 at `5de7215af`: positive passed and both negative controls failed as expected. That receipt does not qualify a changed tuple. |
| Frozen native runtime probes | `a3b1143a878db827dec02e53f076f2b593ecb5b6` | [#4123 arm64 receipt](https://github.com/Takazudo/zudo-front-builder/issues/4123#issuecomment-6431409232): direct-V8/Rolldown `verify.mjs` and `production.mjs` PASS. Preserve the three initial build stops and fourth successful build; the probes passed on their first execution. This is historical source-bound execution, not a rerun on current main or the candidate. |

Observed CI for **main `5de7215af`**, not for the 9.1.1 candidate: [health
40929639882](https://github.com/Takazudo/zudo-front-builder/actions/runs/40929639882),
[binary smoke/scaffold 40929640016](https://github.com/Takazudo/zudo-front-builder/actions/runs/40929640016),
[actionlint 40929640186](https://github.com/Takazudo/zudo-front-builder/actions/runs/40929640186)
and [Security Audit 41405889749](https://github.com/Takazudo/zudo-front-builder/actions/runs/41405889749)
completed with success. The scheduled exam and Supervisor watch need the more precise
interpretations below. No workflow was dispatched or rerun for this refresh.

### Current obligations and next evidence

The complete open inventory is **9 issues**, including dashboard #1492. Existing trackers
remain the owners of their evidence; this ledger creates no replacement issue or scheduler.

| Tracker | Current disposition | Next evidence / boundary |
| --- | --- | --- |
| [#4458 scanner recovery flake](https://github.com/Takazudo/zudo-front-builder/issues/4458) | **Closure-ready, still open.** #4468's tick-scoped assertions and #4491's atomic fixture write are integrated. The scheduled [2026-10-31 exam 41168210490](https://github.com/Takazudo/zudo-front-builder/actions/runs/41168210490), source `5de7215af`, reports 7,055 passed / 87 skipped; both scanner recovery cases pass without retry and the flaky scanner reports no flaky tests. | Manager disposition can use that clean scheduled checkpoint. Skips stay skips; workflow success alone is not the receipt. Original failure and focused Mac evidence remain in the issue. No closure is performed by this ledger refresh. |
| [#4304 Supervisor watch](https://github.com/Takazudo/zudo-front-builder/issues/4304) | **Open; actual verdict red.** Scheduled [2026-11-01 run 41277672402](https://github.com/Takazudo/zudo-front-builder/actions/runs/41277672402), source `5de7215af`, has GitHub conclusion success but `verdict=red rc=2 all=ok:0/0 main=red:25/2`; its green-close step was skipped. Main's identity population spans doc **7.1.0 / 8.1.1 / 9.0.0 / 9.1.0**. | Keep the strict rolling **eight-day** window. Next normal schedule: **2026-11-08 14:37 JST (05:37 UTC)**. A new 9.1.1 transition can prolong drift; green is not promised. No narrowed window, drift allowance, forced dispatch or monitoring-contract change. |
| [#4276 docs/preset acceptance](https://github.com/Takazudo/zudo-front-builder/issues/4276) and [#4576 Neo coordinator](https://github.com/Takazudo/zudo-front-builder/issues/4576) | **Open for the owner's own docs browser confirmation or explicit waiver; neither is recorded.** Current-tuple native preset acceptance, local focused 58/58 specs and agent-driven headed checks locally/deployed are recorded in the [2026-10-30 receipt](https://github.com/Takazudo/zudo-front-builder/issues/4276#issuecomment-6522863052). Deployed docs are `53318efcc`; local source is `5de7215af`. | Owner confirms EN/JA desktop/mobile navigation, long sidebar and Back/Forward, Graphite default and theme persistence, MDX tables/links and console, or explicitly waives. Agent browser checks do not substitute for that owner result. Intel is outside #4576's close condition. |
| [#4123 runtime probes](https://github.com/Takazudo/zudo-front-builder/issues/4123) and [#4366 graph token](https://github.com/Takazudo/zudo-front-builder/issues/4366) | **Native Intel Darwin x64 remains NOT RUN.** Arm64 runtime probes and Windows Stage 7 completion retain their own identities. Arm64 size/admission evidence is a separate obligation and cannot satisfy native Intel execution. | One native Intel session on one recorded source for runtime/browser/production probes and graph_identity/product E2E/Stage 7, or explicit owner narrowing. Rosetta, cross-compilation and arm64 reruns do not satisfy it. No hardware or Mac CI is scheduled here. |
| [#4490 Rolldown external provenance](https://github.com/Takazudo/zudo-front-builder/issues/4490) | Only external-ID/emitted-JS-chunk spelling collision remains; completed cases and shape decisions are historical. No new triggering evidence was found in the issue. | Resume when upstream exposes per-edge external provenance, or a supported ZFB use case demonstrates the collision. No speculative resolver rewrite. |
| [#3357 Cloudflare cf](https://github.com/Takazudo/zudo-front-builder/issues/3357) | Existing upstream-triggered deferral remains. The issue records beta exit or an earlier explicit owner decision as the trigger; CI/migration guidance already exists and is not the blocker. | Revalidate upstream beta status when evaluating that trigger. This refresh does not independently claim a new upstream release check or begin migration/deployment. |
| [#1492 dashboard](https://github.com/Takazudo/zudo-front-builder/issues/1492) | Existing central dashboard, not a new implementation task. Its 2026-10-30 body retains the nine-issue / zero-PR inventory. | Keep evidence in existing issues. Its earlier “scheduled checkpoint” wording for #4458 and #4304 is superseded by the observed runs above. |

**Completed historical lanes:** Deno exit epic [#4386](https://github.com/Takazudo/zudo-front-builder/issues/4386)
and S9/S10/S11 **#4398 / #4399 / #4400 are closed and delivered**. The
[S9 source-bound ledger](https://github.com/Takazudo/zudo-front-builder/issues/4303#issuecomment-6338445544)
retains its NOT RUN items; S11's five-lane notes merged in #4418 (`7a8cff8`).
Direct V8 is the only/default JS host; the old roadmap's default-switch and Deno-removal holds
are historical. This does not erase the CPU-bound interruption/watchdog limitation.
Security alert **#4314 is closed**, recovered in [39624334773](https://github.com/Takazudo/zudo-front-builder/actions/runs/39624334773);
**#4364 is closed**, fixed by #4365 with first-attempt green health on `1531eaef`
([39549166781](https://github.com/Takazudo/zudo-front-builder/actions/runs/39549166781)).
Their original failures below remain evidence, not current unresolved alerts.

Evidence scope: issue bodies/comments and run/job metadata were refreshed directly. Detailed
exam counts and watch identities above come from the parent planning-inspected receipts;
this refresh could not independently download their logs/artifacts because the Actions blob
redirect returned Forbidden. Available artifact identities are `nextest-junit-exam-macos`
**13030810211** and `supervisor-watch` **13072061388** (API reports neither expired).
The issue-status refresh did not rerun these historical jobs or change their completion criteria.

### Docs 9.1.1 candidate verification and admission hold

The docs host uses exact published zudo-doc/history-server 9.1.1; the released ZFB
5.1.5 toolchain and committed published-preset tuple are unchanged. Both upstream
release tags resolve to `81e0e97f6a11185202e63a647a55df3e0cffc50a`.
Registry SHA-512 integrity and every installed package file matched the 9.1.1 tarballs.
The base tsconfig and both generator route templates are byte-identical to 9.1.0;
the original route-adaptation provenance remains unknown.

Early preflight on unchanged main and immediately after the bounded pnpm 12.8.2 lock update:

- Existing contract/main fingerprint: `346c6ad14b7dcde05778dfc1198ba8ac6e1db85235f176e101ec56944a5aed0f`.
- Candidate fingerprint: `47de8f7592f1fb1c463ad9294ea65bbc1f315d50a74e87ab86a129d86eda89b2`.
- Policy 1, parser `yaml@2.9.0` and helper `c1cec11ab7e7d0ea5d5a9548a871dfb886bc05ebbb2ff513d13332429632d787` are unchanged.
- The only changed included record is `snapshots.esbuild@0.28.1.optional: true`.
  The excluded `tsx@4.23.15` snapshot now resolves esbuild 0.28.2 instead of 0.28.1.
  Bounded normal install/update (including depth 0) retained this change.
  `pnpm install --lockfile-only --no-auto-dedupe --prefer-frozen-lockfile`
  avoided unrelated snapshot removal. No lock metadata or contract hash was hand-edited.

**Admission remains pending:** keep both live island-size contracts unchanged until
fresh Linux x64 health evidence and owner-attested native Darwin arm64 workspace/packed
evidence qualify the exact frozen candidate. Preserve the original old-contract rejection,
prove unchanged totals, stage both platforms with `--reason dependency-graph`, review
the paired stage, then obtain the separate owner authorization for `--apply` required by
[LOCAL-NATIVE-EVIDENCE.md](v3-island-size/LOCAL-NATIVE-EVIDENCE.md).
General safe-merge permission does not waive this boundary. This arm64 size evidence is
independent of the existing native Intel obligations.

The focused installed-package baseline on 9.1.0 had 18 passing tests and two new
regression failures; candidate 9.1.1 passes all 20. Node 24.19.0 / pnpm 12.8.2
Cloud checks passed docs type/schema validation, navigation-index synchronization,
the 778-page strict root build and island markers. Browser/CI results, frozen source SHA,
first failures and native handoff belong to the candidate PR's exact-head evidence record;
older-main CI success above does not qualify this candidate. No broad Rust workspace
build is run in Cloud; existing PR-triggered Actions own that verification.

## Historical snapshots — preserved evidence

Everything below is dated history. “Current”, “open”, “held”, “pending”, approval tables and
thread ownership in those records describe their original checkpoint, not today's instructions.
Use the 2026-11-02 checkpoint above for present status; retain older failures and receipts.

Current snapshot: 2026-10-21 03:00 UTC, `main` `cd09e6c` after the v4.5.4 release, PR 4352 and PR 4358;
integration/CI triage owner: the project integration manager (one fixed owner). The "Round record
at main `013b0ab`", "Round record at main `21ef7c4`" and everything from "Historical checkpoint 4294"
onward are earlier records, kept as evidence; where they say "draft", "candidate" or "pending",
the current roadmap below wins. For the JS runtime, the "Deno exit" section below supersedes
the direct-V8 and Deno entries in every older section, including the 2026-10-21 roadmap.

## Historical Deno exit checkpoint (epic 4386, updated 2026-10-22)

Source: `git log --first-parent main` and the
[4386 epic](https://github.com/Takazudo/zudo-front-builder/issues/4386). The owner approved
Gate A and Gate B of 4303 on 2026-10-21. Each step merged into `main` as its own PR with a
regular merge commit.

| Step | Issues | State |
| --- | --- | --- |
| S1 vendored ICU, S2 `cargo-removal` reason, S3 qualification rows | 4387, 4388, 4389 | Merged in PR 4401 (`addf681`) |
| S4a/S4b default flip to `direct-v8`, review fixes, rollback lane | 4390, 4391 | Merged in PR 4403 (`978f7e7`) |
| S5 Gate A confirmation on `main` | 4392 | Closed (confirmation only, no code) |
| S6a/S6b port or move the `embed_v8`-only tests | 4393, 4394 | Merged in PR 4406 (`2abb238`) |
| S7a/S7b/S8 remove the deno_core host and `embed_v8`, removal declaration, Linux stage, Darwin re-pin and paired apply | 4395, 4396, 4397 | Merged in PR 4412 as `7e7aea9` on 2026-10-22 (08:38 UTC) |
| S9 Deno-free proof and final evidence ledger | 4398 | In progress |
| S10 docs | 4399 | In progress |
| S11 release-notes hand-off | 4400 | Open; waits on S9 and S10 |

- `direct-v8` is now the only JS runtime and a default feature; `--no-default-features` builds
  without V8. The deno_core host, the `embed_v8` feature and the rollback lane are gone, so
  rolling back means reverting PRs, not switching a feature.
- Released binaries up to v4.6.0 still use the deno_core host. The release that first ships the
  direct host is the owner's decision; nothing here approves a release, publish or tag.
- Known limitations carried forward: CPU-bound JavaScript is not interruptible, and the
  qualification harness's watchdog drain is best-effort (4289, see
  `prototypes/direct-v8/WATCHDOG-4289.md`).

## Historical roadmap (main `cd09e6c`, updated 2026-10-21 03:00 UTC)

Source: `git ls-remote`, the GitHub API and the [4336 coordination issue](https://github.com/Takazudo/zudo-front-builder/issues/4336).
Where this section and the older sections below disagree, this section wins.

| Lane | Exact head | State and evidence | Held |
| --- | --- | --- | --- |
| `main` (production boundary) | `cd09e6cdfe9e2d9bb8f5bef34eacca4ef4dc73e1`; latest tag `v4.5.4` on `ea90fe12` | v4.5.4 published (all 11 packages on `latest`). Since the tag: PR 4352 (`7852d60b`, test-only) and PR 4358 (`cd09e6c`, fixes for 4353 and 4354). Push CI at `cd09e6c`: binary smoke/scaffold and deploy success, **health red** ([run 39497401813](https://github.com/Takazudo/zudo-front-builder/actions/runs/39497401813)) on a 5 s vitest timeout in an island-size end-to-end case, tracked in [4364](https://github.com/Takazudo/zudo-front-builder/issues/4364) with the fix in [4365](https://github.com/Takazudo/zudo-front-builder/pull/4365). Health at `7852d60b` and `ea90fe12` was green | Release, publish, tag; admission or live contract change |
| Experimental runtime (direct V8, Rolldown) | `base/4301-direct-v8` = `a56c91f5f1abc83557ea57cfe1e75f009a61374e` | After `f5c4b48`: 4361 (`268c3402`, local-evidence tooling) and 4340 (`a56c91f`, Rolldown Windows verbatim entry). CI on `a56c91f` itself is pending. P2a: paired Linux+Darwin stage passed and `--apply` ran on the Mac; draft 4362 carries the applied contracts. Open drafts on this base: 4341, 4344, 4346, 4359, 4360, 4362; 4345 is stacked on the 4340 branch | Main integration of direct V8; lock admission; default-engine switch |
| Security stage | `base/4314-security-darwin-evidence` = `7e79458304878d51bafd29e24ec25e93b84893ca` | Unchanged. Drafts 4332 and 4317. Security Audit stays red on `main` (rustls) | Live apply; admission; 4317 merge |
| Native qualification | main `7852d60b` | 4294: native macOS arm64 full workspace 6877/6877 passed, doctests green, first attempt ([receipt](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6301282943)). 4133: not reproduced in 5 predeclared libtest/nextest runs. 4231 item (2): rows a, b, c, g, h pass on the Mac; d (Darwin x64), e and f (Windows) are not run | Sharding drafts 4305/4325; darwin-x64 |

## Round record at main `013b0ab` (2026-10-20, before the v4.5.4 release)

### Five-lane state (2026-10-20 10:40 UTC, from the 4336 issue body)

Source: the [4336 coordination issue](https://github.com/Takazudo/zudo-front-builder/issues/4336) body,
checked there with `git fetch` and the GitHub API. The batch plan is
[BATCH-ZFB-01](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6288828584),
with owners assigned in [ZFB-S006](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6288873766).
The Codex takeover request was withdrawn
([correction](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6288684799));
the existing owners continue.

| Lane | Exact head | State and evidence | Held |
| --- | --- | --- | --- |
| 1. `main` (production boundary) | `013b0ab8fe5059e64ac0f7592a6eb938ac476e29`; latest tag `v4.5.3` | The approved batch 4323, 4328, 4329, 4324 is integrated (merge SHAs in the table below). Push CI at `013b0ab` attempt 1: health, smoke and actionlint success. Health and smoke at `7426913` and `695ca66` were cancelled by later pushes, not passed. Unmerged drafts on `main`: 4339 (L4, `e95b841`), this ledger 4330, 4334 (`a733abf`), 4333 (bump probe, `82b7111`), 4307 (`14d984a`, contents already on `base/4301-direct-v8`), 4306 (`5162d75`, contained in 4307) | Any new `main` merge |
| 2. Experimental runtime (direct V8, Rolldown) | `base/4301-direct-v8` = `f5c4b48bddd76448b87ef9b347ae45b7e1d94c52` (4338 merge, parents `9b4db03`, `923ab24`) | Integrated first-parent: 4307 (`bea94d1`), 4331 (`25054ed`), 4335 (`c437463`), 4337 (`9b4db03`), 4338 (`f5c4b48`). Its Cargo.lock (`07fa6450…e4f9`) differs from `main`'s and is not admitted, so health's two island-size-budget tests are red. Direct-V8 plain rows at 4338 head `923ab24` ([run 39394691310](https://github.com/Takazudo/zudo-front-builder/actions/runs/39394691310)): linux-x64, linux-arm64 and windows-x64 PASS (Windows lifecycle 6/6). Draft [4340](https://github.com/Takazudo/zudo-front-builder/pull/4340) (Rolldown owner, `8c8d676`, base `f5c4b48`): [run 39403120807](https://github.com/Takazudo/zudo-front-builder/actions/runs/39403120807) attempt 1 has lifecycle FAIL on all three rows; Windows build and dev pass and `UNRESOLVED_ENTRY` is gone; Linux x64 and arm64 fail on the shared DCE `Export 'routes'` error, first seen at `349b112c` (run 39376525150). The DCE patch is unpushed and preserved ([ZFB-RD-007](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6288895035)); the 08:40 prohibition still stands until the owner states otherwise on 4336, and the denied operation is not retried | 4340 merge; DCE patch adoption; lock admission; default-engine switch |
| 3. Security stage | `base/4314-security-darwin-evidence` = `7e79458304878d51bafd29e24ec25e93b84893ca`, not on `main` | Integrated: 4315, 4319, 4326. Rustls lock `56319425…f5bd`, separate from the direct-V8 lock. Open drafts: 4332 (`8bb9003`), 4317 (`9e8fbd1`, base old `21ef7c4`). Measured SHA `bda633b3`, tool/base `7e79458`. Saved Linux artifact 12213032790 expires 2026-10-26 18:46:58 UTC; L1 confirms it. D1 to D3 ([ZFB-D001](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6283407479)) still wait on the owner | Live apply; admission; security attestation; 4317 merge |
| 4. Frozen native qualification | Darwin arm64 direct-V8 row at `9b4db03` only | Neo at `9b4db03`: PASS 10/10, lifecycle 6/6, graph 476, focused 49/16/14, offline 142; the first stage 3 failed on a missing `pnpm install` ([6284829340](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6284829340)). That result is evidence for `9b4db03` only. The rev3 recipe for `f5c4b48` (offline 147) is prepared only; Mac qualification at `f5c4b48` is NOT RUN and Neo is stopped. 4294: the native first run is 6,854 tests at `21d6807e` (6,838 passed, 7 failed, 9 timed out); no full native run at `013b0ab`. 4231 item (2): (a) and (g) PASS on Moya's Mac; (b) to (f) and (h) NOT RUN ([ZFB-Q002](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6283408930) d and e unanswered) | New Mac work; sharding 4305/4325 |
| 5. Release preparation | 4334 `a733abf` (two commits ahead of `013b0ab`); 4333 `82b7111` (bump probe, do not merge) | 4.5.4 preparation is not release approval. L5 reconciles the five package notes to the changes actually merged (batch item E) | Release, publish, tag, deploy |

Not approved, unchanged: a `main` merge, release/publish/deploy, any lock, pin, admission, ceiling or
assertion change, security attestation or live apply, the 4317 merge, sharding 4305/4325, new Mac
work, the 4340 merge, a default-engine switch, and declaring Deno exit or shipping qualification done.

### Main integration of 2026-10-19 (owner-approved)

The owner approved this order at 19:26 UTC. Each PR was merged with a normal merge commit, with no
squash, rebase or force-push, after its latest head showed every required check green. The ruleset
does not require an up-to-date head, and none of the four PRs touched another one's files.

| Order | PR | Tested head (required CI green) | Merge commit on `main` |
| --- | --- | --- | --- |
| 1 | [4323](https://github.com/Takazudo/zudo-front-builder/pull/4323) roadmap ledger | `ccf7363` | `7f58a01607db44c418ec68b08746d806bdc553da` |
| 2 | [4328](https://github.com/Takazudo/zudo-front-builder/pull/4328) packed-build byte parity (4231) | `732a03de` | `7426913076cf6ef76b948281b2f78a8903e3c668` |
| 3 | [4329](https://github.com/Takazudo/zudo-front-builder/pull/4329) docs cleanup and v4.5.4 drafts | `ed55aaa` | `695ca660c3b166e5a43bc0370b35c1eb63e7dcd5` |
| 4 | [4324](https://github.com/Takazudo/zudo-front-builder/pull/4324) nextest 0.9.140 pin | `7aeb31f` | `013b0ab8fe5059e64ac0f7592a6eb938ac476e29` |

Automatic `main` CI, first attempt, no retries or dispatches:

| `main` SHA | health | Binary smoke, scaffold E2E and showcase | Other |
| --- | --- | --- | --- |
| `7f58a01` | [39315505385](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315505385) cancelled (superseded) | [39315505689](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315505689) success | — |
| `7426913` | [39315529982](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315529982) cancelled (superseded) | [39315529777](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315529777) cancelled (superseded) | — |
| `695ca66` | [39316341673](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341673) cancelled (superseded) | [39316341725](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341725) cancelled (superseded) | docs-deploy [39316341746](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341746) success |
| `013b0ab` | [39316381257](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381257) success (completed 20:47 UTC; `build (no-v8)`, `health` with workspace nextest on the 0.9.140 pin, and all seven `wasm-md` legs) | [39316381226](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381226) success | actionlint [39316381210](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381210) success |

- Cancelled runs are not passes. `main` runs are superseded by the next push, so the intermediate
  heads have no complete health result. `013b0ab` contains all four merges, and its run is the
  integrated evidence.
- At `013b0ab` the smoke run passed both binary builds, the four local-mode smokes, the Amazon
  Linux glibc floor, `Scaffold E2E` (now including the 4328 Stage 7 parity check) and the built-site
  browser smoke. `Showcase deploy (create-zfb, production)` deployed and passed its post-deploy
  health check (job 118285903814).
- The docs deploy at `695ca66` published the merged README/installation changes. The v4.5.4
  drafts live under `research/`, so no version page was published.
- Still not run at `013b0ab`: `exam.yml` with the 0.9.140 pin (next scheduled Sat 2026-10-24),
  any native Mac or Windows run, and Security Audit (last red at `21d6807e`, see 4314).

### Owner policy from 20:38 UTC: Claude-led, implementation first

Claude leads and keeps implementing. Release preparation must not stop implementation. When the
Mac is free, verified changes are gathered into a release checkpoint that downstream projects can
dogfood. This is release planning, not blanket publish approval. Before any actual release, report
the exact version, the included changes and the remaining qualification. The owner is away for
about six hours from 20:33 UTC. Decisions that need new approval are collected below and are not
inferred. The message did not ask for additional Mac tests.

Two separate tracks, which must not be mixed:

- **4.5.4 patch checkpoint (now; L5).** The version is approved as 4.5.4; the release is not. The
  candidate is `main` at its latest CI-green head, currently `013b0ab`. The drafts and the bump table are in `research/release-prep-v4.5.4/README.md`. Package-facing
  content is the two islands fixes (4298, 4308). The rustls 0.23.46 transition is not on `main` and
  is not in the drafts. L5 is preparing the checkpoint report and a draft bump PR. md-wasm sizes and
  island-size ceilings come from CI measurement; a red ceiling is recorded, never relaxed.
- **Deno exit roadmap (future; 4303, not assigned).** Superseded: the Deno exit was approved
  and merged on 2026-10-22; see "Deno exit" at the top. As recorded then: nothing in the current plan removes Deno or
  switches the default. The remaining blockers are listed in the
  [4303 comment](https://github.com/Takazudo/zudo-front-builder/issues/4303#issuecomment-6276732610):
  the Gate A default-switch approval, native qualification on four platforms, lock-admission apply,
  the deno_core crates still on the default path, a missing direct-V8 CI lane (L2 is drafting an
  opt-in, non-required one), and a separate Gate B approval for removal. The work that does
  proceed is opt-in only: 4307 and the 4302 Linux x64 row and harness.

### Awaiting owner approval (held, not inferred)

| Decision | State and evidence |
| --- | --- |
| One apply/merge approval request based on the 4314 reconciliation plan | Plan: [4314 comment](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6277405812) (same content in the project files at `l1-security-evidence/reconciliation-plan-4314.md`). Decision points: D1, whether the owner attests the neutral receipt N as a new receipt (without it, a public Darwin apply is not possible with the current tooling); D2, the `actualGzip` assertion form in the Linux history test; D3, whether the operator string may be published. Deadline: the Linux artifact expires 2026-10-26. The plan covers (1) the README basis/live hash mismatch, (2) the apply order and collisions where Darwin and Linux share the README and test file, and (3) the historical test updates (`repinnedFrom` and others) with fresh-context review. It names exact SHAs/branches, changed files, stage order, verification, stop conditions and rollback, keeps raw evidence private and separates original from derived hashes, without relaxing any gate or admission condition. The owner will decide on one consolidated request, with verified facts and inferences kept apart. Until then Darwin apply, Linux re-stage and apply, the `main` integration and the release all stay unapproved |
| Darwin apply for the rustls lock transition (4314), which needs a Mac operation | Stage attempt 2 matched the recipe's expected values, as checked from Moya's reported values ([L1 verdict](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276677595)). Apply waits for an independent review of `staged.diff` (SHA-256 `942fd70b…`), which needs that file to be shared first. `--apply` and live admission are not approved. Apply must also update the two hand-pinned history tests (`repinnedFrom` and the rewire route) and get a fresh-context review. Route settled to one: `--apply` of a local-native stage re-verifies that the measured SOURCE (`bda633b`) is HEAD of a clean checkout (`scripts/island-size-local-evidence.mjs:205-256`), so a direct apply on the base or `main` is impossible by design. The route: on the Mac, apply from a clean `bda633b` checkout; return the three files' bytes and hashes (no push from the Mac); L1 commits them on a branch cut from `bda633b` and merges it into the base normally. Apply plan in [4314 comment](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6277260610); recipe fix in [4332](https://github.com/Takazudo/zudo-front-builder/pull/4332) commit `8bb90031`. Both stages write the same two files (the README and the test file), so after Darwin apply the Linux stage must be re-run from the same artifact (expires 2026-10-26) |
| Linux x64 lock transition apply (4314) | `decision-linux-x64.json` still pins the old lock, so Darwin alone does not turn the base green (same comment). The Linux stage is done in Cloud by integration-lead decision, within the approved stage-only scope: exit 0 ([4314 comment](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276966739); evidence in draft [4332](https://github.com/Takazudo/zudo-front-builder/pull/4332) under `stage-evidence/linux-4314/`, manifest `9b2746b9…`, diff `fddf15d5…`). Apply stays held. Apply must also update the two hand-pinned history tests (`repinnedFrom` and the rewire route) and get a fresh-context review |
| Handle the local path in [4314 comment 6276588952](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276588952) | Edit the body or delete the revision; unchanged until the owner decides (see the incident note below the table) |
| Merge the security base into `main` | `base/4314-security-darwin-evidence` (4315, 4319, 4326) reaches `main` only after both applies, together with the other candidates; not approved |
| Merge 4317 (hosted Linux exact-source lane) | [PR 4317](https://github.com/Takazudo/zudo-front-builder/pull/4317), draft on `base/4314-security-darwin-evidence`; run 39248247275 passed for source `bda633b` only |
| Adopt 4305 native sharding (with 4325) | On hold until after the next exam (owner, 19:26 UTC). [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305) and [4325](https://github.com/Takazudo/zudo-front-builder/pull/4325) stay draft; 4325 needs `main` merged in after 4324 because of the `exam.yml` install-line conflict |
| Whether rustls 0.23.46 must be on `main` before the 4.5.4 cut | Prepared security entry and the reasoning: `research/release-prep-v4.5.4/README.md`, section "Security lane: not on `main`, not in these notes". 4314 stays red until the transition lands |
| 4231 item (2): Darwin, Windows and Mac WebKit checks of the 4298 token change | Rows (a) and (g) PASS on the Mac; the other six rows (b–f, h) wait on owner decisions and stay NOT RUN; nothing waived or closed. (g): Moya ran `pnpm test:webkit-back` once at frozen `013b0ab` in a dedicated clean detached checkout: 4 selected / 4 PASS / 0 failed / 0 skipped, retries 0 (3 back navigations plus 1 native History-quota rejection/recovery, 10.5 s under a 12 s guard; Playwright 1.61.0, WebKit 26.5 revision 2311, Node 24.14.0, pnpm 12.8.2, Apple M4 arm64, macOS 26.6.2). Evidence is kept private by Moya (receipt SHA-256 `ccbb2f65…`, `webkit.log` SHA-256 `aca12259…`). It covers only those four scenarios and is not reused for (h), Windows or Darwin product e2e. (a): Moya ran the 20 selected tests (13 lib + 7 integration, confirmed by a prior `nextest list`) once at frozen `013b0ab` in a dedicated clean checkout, using L4's recipe ([4231 comment](https://github.com/Takazudo/zudo-front-builder/issues/4231#issuecomment-6277825566)): 20 PASS / 0 failures / 0 selected skips, retries 0; the 394 skipped are outside the filter, not skips of selected tests (Apple M4 arm64, macOS 26.6.2, Rust/Cargo 1.99.1, nextest 0.9.140, Node 24.14.0, esbuild 0.25.12; locked/offline, JUnit, serial under the heavy guard). Evidence is kept private by Moya (receipt SHA-256 `e30674a9…`, JUnit SHA-256 `a01e8fdc…`) and covers only (a). Earlier, a read-only `cargo tree --locked --offline -p zfb-islands` showed no deno_core/v8 dependency, which is not a test pass.  Item (1) is met by 4328 (`7426913`). L4's remaining-verification ledger: [4231 comment](https://github.com/Takazudo/zudo-front-builder/issues/4231#issuecomment-6277543537) (same content in the project files at `l4-issue-sweep/4231-remaining-verification-ledger.md`), candidate SHA `013b0ab8fe5059e64ac0f7592a6eb938ac476e29`. Runnable on the Mac (2 rows): (a) Darwin arm64 `graph_identity`; (g) macOS WebKit `test:webkit-back`. Owner decisions (6 rows): (b) the route for 19 Darwin e2e tests, one of three; (c) whether the linux-x64-pinned script needs a Stage 7-equivalent change; (d) whether Darwin x64 hardware exists; (e) whether a Windows environment exists; (f) Windows e2e is 0 tests under `cfg(unix)`; (h) the WebKit token behavior needs a new test. 4231 stays open until then, and 4252, 4243 and 4241 wait on 4231 in that order |
| Add a macOS runner leg to the direct-V8 lane (4331) | Not approved. The lane runs on standard ubuntu runners; Linux arm64 and Windows standard-runner legs are allowed by integration-lead decision, macOS is held for the owner |
| Run the 4.5.4 release | Not approved. Merging drafts 4333 and 4334 (both CI green) and the timing of the native qualification are decided together with it. Before asking, report the exact version, the included changes and the remaining qualification (native Mac at the candidate SHA, the Darwin and Linux applies, 4305 on hold) |

**Evidence handling (owner, 2026-10-19 21:05 UTC; supersedes the 21:02 decision-card choice of
option (a), which is withdrawn):** raw evidence (receipts, bundles, stage originals) stays private
and unchanged, and embedded personal absolute paths are not published. Option (a) publication is
not approved. A public version may be a sanitized derived report that states the transformation
steps, the replaced ranges and its relation to the original, with the original artifact hashes kept
separate from the derived hashes. A receipt must never be rewritten silently and still claim the
original hash. If needed, a neutral receipt is created as a new identity, never treated as the
original. This is not an instruction to reissue the original receipt or to re-stage.

**Incident (2026-10-19):** (1) The first version of L1's plan comment on 4314
([comment 6277405812](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6277405812))
included the operator string by mistake. It is not a credential, only a display name and a
description of the run environment, and it appeared only in that first version. The current body no
longer has it, but GitHub's edit history still does (not yet handled); only a repository
administrator can delete that revision. A scan of every body on 4314 and 4332 found zero operator
values and zero credential-shaped strings. (2) The current body of
[4314 comment 6276588952](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276588952)
(20:27 UTC) still contains one local absolute path with a user name; it is unchanged and waits on the
owner (edit the body or delete the revision). (3) The private project-files review note
`darwin-stage-02-review.md` contains the operator value; it is not published and is unchanged.

The 4252 S7 ledger is that issue's own deliverable, so the missing per-PR merge SHAs and CI for
S0, S1 and S2 and the post-merge `main` CI for `15a76210` (noted by L4 on 4252) are recorded there
when 4252 is worked, not copied into this roadmap.

Also still held from earlier rounds: 4268 (noyalib 0.0.58 `render` is over its ceiling), 4289 and
4293 (drain contract), 4311 (provenance policy), and the 4303 gates above.

### Threads, owners and open draft PRs

| Lane | Owner | Current work | Open PRs |
| --- | --- | --- | --- |
| L0 integration, CI triage | roadmap thread | This ledger; `main` CI at `013b0ab` | this ledger PR (draft) |
| L1 security, Darwin evidence | L1 thread | Review `staged.diff` once shared; Linux stage done in Cloud by integration-lead decision (evidence in draft [4332](https://github.com/Takazudo/zudo-front-builder/pull/4332)); no apply | [4317](https://github.com/Takazudo/zudo-front-builder/pull/4317) (draft, base `base/4314-security-darwin-evidence`) |
| L2 opt-in direct V8 | L2 thread | 4331, 4335, 4337 and 4338 are integrated into `base/4301-direct-v8` (`f5c4b48`); see lane 2 above. Batch item B3: inventory 4302 coverage. The macOS runner leg stays awaiting approval | [4307](https://github.com/Takazudo/zudo-front-builder/pull/4307) (draft; contents integrated) |
| Rolldown | Rolldown thread | [4340](https://github.com/Takazudo/zudo-front-builder/pull/4340) at `8c8d676`: lifecycle FAIL on all three rows (DCE). Unpushed DCE patch preserved, held on the owner's word (batch items B1, B2, B4) | [4340](https://github.com/Takazudo/zudo-front-builder/pull/4340) (draft) |
| L3 native qualification | L3 thread | `exam.yml` on `main` installs nextest 0.9.140 via `taiki-e/install-action` v2.82.10 (4324, `013b0ab`); the first exam on it is the scheduled run on 2026-10-24 04:17 UTC, and no exam has run since 4324 ([L3 report](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6288908463)). 4305 and 4325 stay draft until after that exam. 4133, 4123 and 4124 stay deferred | [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305), [4325](https://github.com/Takazudo/zudo-front-builder/pull/4325) (drafts, on hold) |
| L4 issue sweep | L4 thread | 4019 closed with evidence; 4231, 4252, 4243 and 4241 held on 4231 item (2) | — |
| L5 docs, release prep | L5 thread | 4.5.4 checkpoint report and draft bump PR. Measurements on [4333](https://github.com/Takazudo/zudo-front-builder/pull/4333) need no ceiling change: the four md-wasm artifacts are byte-identical to 4.5.3, `render` is 1,018 B under its ceiling, and the Linux island-size raw and gzipNormalized totals are unchanged. Two lineage assertions in the island-size budget test were rewritten for the new lineage; a fresh-context review of that gate file was done and approved ([4333 comment](https://github.com/Takazudo/zudo-front-builder/pull/4333#issuecomment-6277621735)): ceilings and allowance are byte-identical, nothing relaxed, and its two low-severity findings were applied in `82b7111`. Both checkpoint drafts are all green and unmerged: 4333 (version bump, head `82b7111`; [health 39327073260](https://github.com/Takazudo/zudo-front-builder/actions/runs/39327073260) passed with the 4.5.4 size record and the island-size contract, no ceiling change) and 4334 (checkpoint report, head `a733abf`) | [4333](https://github.com/Takazudo/zudo-front-builder/pull/4333), [4334](https://github.com/Takazudo/zudo-front-builder/pull/4334) (drafts, CI green; merges decided together with the release approval) |

Other open drafts not owned by a running thread: [4306](https://github.com/Takazudo/zudo-front-builder/pull/4306)
(superseded; contained in 4307) and [4293](https://github.com/Takazudo/zudo-front-builder/pull/4293)
(HOLD, 4289 drain contract).

## Round record at main `21ef7c4` (before the 2026-10-19 main integration)

### Cloud round summary (2026-10-19)

All Cloud-doable work from this round is done. States below use the ledger vocabulary: implemented,
reviewed, integrated (into which branch) and verified (which evidence, which SHA). Nothing here is
released, applied to live admission, or merged to `main` beyond PR 4322.

**1. Outcomes by lane**

| Lane | Implemented + reviewed | Integrated | Verified | Not run |
| --- | --- | --- | --- | --- |
| L0 roadmap | This ledger (PR 4323), 4235 body refresh | Draft PR to `main` | Docs format check; PR CI green at `b1268ec` | — |
| L1 security | 4315 (rustls 0.23.46), 4319 + 4311 unified as 4326 | `base/4314-security-darwin-evidence` `7e79458` | Hosted Linux run 39248247275 (source `bda633b`, log and artifact checked separately) | Darwin collection, stage+apply; base health keeps 2 known reds until apply |
| L2 direct V8 | 4307 refreshed onto main, Wind manifest regenerated, 4302 harness (9 review fixes) | `base/4301-direct-v8` `bea94d1` | Linux x64 native row PASS at `14d984a`; CI red only on unadmitted lock | Other native rows, Windows observer, packaging, Rolldown coverage, frozen acceptance, Chromium, default local build |
| L3 native | 4324 (nextest 0.9.140 pin, install-action v2.82.10), 4325 (4305 fixtures at 0.9.140) | Drafts only | 4324 CI green, 6,874 passed; health now installs nextest from the tarball with SHA-256 check; Mac helper receipt at `21ef7c4` (graph lane 43/43) | `exam.yml` with the new pin not yet run (next scheduled Sat 2026-10-24); native full workspace at `21ef7c4` |
| L4 sweep | Classification, closures, 4231 test (4328) | 4328 ready for review | 4328 CI 15 success / 0 failure at `732a03de` | — |
| L5 docs/release prep | 4329: current-state README, toolchain/Python 3.11 docs, five-lane v4.5.4 drafts, bump proposal | Draft | Docs build passed; health still running at this update | No release, tag or publish |

**2. Issues**

- Closed as completed: 4251, 4219, 4218, 4224, 4223, 4098 (evidence comments on each); 4311 closed as
  integrated into 4326; probe PR 4327 closed after recording its size result on 4268.
- Open with next action: 4231/4252/4243/4241 (merge 4328, then S7 ledger, then close in that order);
  4314 (close after rustls reaches `main` and a Security Audit run is green); 4294 (native full
  workspace at a selected SHA); 4133 (Mac reproduction); 4301/4302 (lock admission, then native
  rows); 4268 (owner decision; 0.0.58 over ceiling); 4304 (watch 2026-10-25); 4019 (real supervisor
  suite on a reaping init); 4289/4293, 4303, 318, 3386, 3372, 3879, 3357, 1373 (owner or upstream
  triggers as classified above); 4123/4124/4276 (Mac or owner manual); 1492 sticky.

**3. Main candidates and recommended order**

1. **4323** (this ledger; docs-only; conflict-free with the others).
2. **4328** (packed-build byte comparison for 4231; CI green; ready). Unblocks the 4231 chain.
3. **4329** (docs and v4.5.4 drafts; drafts live outside the changelog path so a `main` push
   publishes no version page). Merge after its health finishes green.
4. **4324** (nextest 0.9.140 pin). Merge before any 4305 work; first `exam.yml` run with it is the
   2026-10-24 schedule unless the owner dispatches one.
5. **4305 + 4325** only if the owner adopts sharding: rebase 4325 onto `main` after 4324, because
   both edit the `exam.yml` nextest install lines (merge-tree conflict confirmed); keep 4324's
   install-action v2.82.10 pin.

Not candidates now: `base/4314-security-darwin-evidence` (red until stage+apply is approved and
done; 4317 rides with it) and `base/4301-direct-v8` (waits on lock admission, which depends on L1).

**4. Minimal Mac prompts**

| Prompt | Fixed SHAs | Status |
| --- | --- | --- |
| L1 `/mnt/project-files/l1-security-evidence/mac-prompt-4314.md` (stage only, `LOCAL-SECURITY-4314.md`) | SOURCE `bda633b`, TOOL `7e79458` | Waits for the owner to lift the Mac preflight hold |
| L3 [4294 comment](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6274028064) (new capture helper) | `21ef7c4` | Executed by Moya: [receipt](https://github.com/Takazudo/zudo-front-builder/pull/4322#issuecomment-6274889047) |
| L2 [4302 native-row handoff](https://github.com/Takazudo/zudo-front-builder/issues/4302#issuecomment-6275155771) (`PLATFORMS.md` "Native row run prompts") | CANDIDATE `14d984a` | Not authorized or run |

**Owner decisions recorded 2026-10-19 19:26 UTC:** merge 4323 → 4328 → 4329 → 4324 to `main` after
each PR's required CI is green; 4305/4325 sharding adoption is on hold until after the next exam;
Mac security is approved for recipe/resource preflight then stage collection only (no `--apply`, no
live admission); prepare version 4.5.4 (patch) notes and bump, with the release itself not approved.

**5. Remaining conditions for release and owner decisions to take together**

- Lock admission apply (rustls transition on L1; direct-V8 edge on L2): Darwin stage collection is approved, `--apply` is not.
- Whether to adopt 4305 sharding (with 4325): on hold until after the next exam.
- Version: 4.5.4 patch chosen by the owner; the release itself is not approved.
- Whether rustls 0.23.46 must be on `main` before the cut (4314 stays red until it is).
- Native verification before or after the cut (full workspace at the candidate SHA is not run).
- Whether to merge 4317 (hosted Linux exact-source lane).

### Main and its CI

- `main` = `21ef7c4678adba115a3607801ededeeaf6e5814e` (merge of PR [4322](https://github.com/Takazudo/zudo-front-builder/pull/4322)).
  It contains PR 4312 (checkpoint: 4308, 4309, 4313, 4316, 4318; merge `502d9e6e`), PR 4310
  (external-edge admission capability, head `28be51b`), PR 4321 (capture failure handling, `ac5eedd`)
  and PR 4320 (capture TMPDIR fixture, `35f7b86`). Earlier merged work referenced below: 4298
  (`15a76210`), 4234 (`d28cdd57`), 4259, 4260, 4120 (`8b1fe8b0`).
- Exact-head automatic CI on `21ef7c4`, first attempt, no retries:
  [health 39294618774](https://github.com/Takazudo/zudo-front-builder/actions/runs/39294618774) success
  (`build (no-v8)`, `health`; `wasm-md` skipped by its path gate) and
  [binary/scaffold/showcase 39294618739](https://github.com/Takazudo/zudo-front-builder/actions/runs/39294618739) success.
  No docs-deploy or wind-computed-style run was triggered for this head. This is Linux evidence only.
- Not yet run at `21ef7c4`: any native Mac/Windows execution, the scheduled `exam.yml` (last run
  39002778546 at `d1c8b288`; next scheduled Sat 2026-10-24), Security Audit (last run 39217854663
  red at `21d6807e`, see 4314), Supervisor watch (next Sun 2026-10-25), YAML candidate watch (next Thu 2026-10-22).
- Cloud host at snapshot: 4 vCPU, 15 GiB RAM, about 30 GiB writable disk allowance (`df` shows
  9.1G used / 30G available). That is the same class of limit that stopped a full Rust build in
  PR 4315's receipt; do not repeat a cold full workspace build here. Use hosted Linux Actions.

### Lanes, owners and next actions

One owner per lane; files listed are the lane's edit scope. A lane must not edit another lane's
files without a recorded hand-off. `research/development-queue.md` itself is edited only by the
integration manager.

| Lane | Items | Branch / head | Edit scope | Depends on | Next action |
| --- | --- | --- | --- | --- | --- |
| L0 Integration + CI triage | main, this ledger | `main` `21ef7c4` | this file; `base/**` aggregation branches | — | Create a fresh `base/**` from `main` for the next batch; record exact-head CI per integrated head |
| L1 Security + Darwin evidence | [4314](https://github.com/Takazudo/zudo-front-builder/issues/4314); 4315, 4319 and unified-module 4326 integrated, 4311 closed as integrated; [4317](https://github.com/Takazudo/zudo-front-builder/pull/4317) open on the new base | `base/4314-security-darwin-evidence` `7e79458` (source `bda633b` unchanged) | `Cargo.lock`; `research/v3-island-size/**`; `scripts/{rustls-security-transition,island-size-local-evidence,rebaseline-island-size-darwin,rebaseline-island-size-linux}.mjs` + tests; `.github/workflows/{island-size-darwin,local-darwin-security-evidence}.yml` | Owner Darwin collection and stage+apply approval | Base health has two known reds (old lock-pin assertions; Linux lock provenance) that stay until stage+apply. Run 39248247275 log and artifact checked separately and recorded on 4317 (zip retained). Mac prompt: `/mnt/project-files/l1-security-evidence/mac-prompt-4314.md` (SOURCE `bda633b`, TOOL `7e79458`) |
| L2 Opt-in direct V8 | [4301](https://github.com/Takazudo/zudo-front-builder/issues/4301), [4307](https://github.com/Takazudo/zudo-front-builder/pull/4307) (contains 4306 as `322ad117`), [4302](https://github.com/Takazudo/zudo-front-builder/issues/4302) harness | 4307 head `14d984a` (main merged at `bd4feaf`); integrated by normal merge into `base/4301-direct-v8` `bea94d1` (not a main candidate) | `crates/zfb*/**` touched by 4307; `prototypes/direct-v8/**`; `docs/public/wind-examples/manifest.json` (regenerate only) | Lock admission: declaration, receipt and dry proof PASS on both targets; apply waits on Darwin evidence, L1 and owner approval. 4310 capability alone does not admit it | CI on 4307: 25 success / 1 failure / 7 skipped; the failure is only health's unadmitted-lock assertion (main is green); cause, fix owner (L1) and next check in [4307 comment](https://github.com/Takazudo/zudo-front-builder/pull/4307#issuecomment-6275153813). Wind manifest regenerated with the official generator (173 fresh). 4302 harness (`PLATFORMS.md`, `qualify-target.py`; 9 review findings fixed); Linux x64 native row PASS; fixed native-row procedure and minimal Rolldown failing case in [4302 comment](https://github.com/Takazudo/zudo-front-builder/issues/4302#issuecomment-6275155771). NOT RUN: Linux arm64, Darwin arm64/x64 and Windows native rows, Windows observer, packaging harness, Rolldown coverage, frozen producer acceptance at this head, Chromium, default (`embed_v8`) local build |
| L3 Native qualification | [4294](https://github.com/Takazudo/zudo-front-builder/issues/4294), [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305), [4133](https://github.com/Takazudo/zudo-front-builder/issues/4133); drafts [4324](https://github.com/Takazudo/zudo-front-builder/pull/4324), [4325](https://github.com/Takazudo/zudo-front-builder/pull/4325) | 4324 `7aeb31f` (base main); 4325 `9e14d59` (base 4305 branch); 4305 `bd02b8a` | `.github/workflows/{health,exam}.yml` nextest pins; `.github/actions/native-exam-setup/**`, `scripts/native-exam*`, `tests/unit/native-exam.*`, `tests/fixtures/native-exam/**` | Mac owner; 4305 adoption by owner | Decision: unify on nextest 0.9.140 (owner, 2026-10-19). Before it, four versions coexisted (4305 0.9.148; `health.yml` 0.9.138; `exam.yml` 0.9.136 via the `taiki-e/install-action` v2.79.5 pin, seen in run 39002778546; Mac runs and capture helper 0.9.140). 4324 pins health/exam to 0.9.140 and bumps `taiki-e/install-action` to v2.82.10 (health now installs nextest from the release tarball with a SHA-256 check; CI all green, 6,874 passed; merged as `013b0ab`; `exam.yml` now uses 0.9.140 via v2.82.10 and its first exam is 2026-10-24); 4325 re-takes 4305's fixtures and conflicts with 4324 on the `exam.yml` install lines. Minimal Mac prompt: [4294 comment](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6274028064) |
| L4 Issue sweep | items listed below | new topic branch per change | issue comments; new test for 4231 | — | Closures done (see sweep); 4231 test in 4328 (main candidate); S7 ledger 4252 after it merges |
| L5 Docs and release prep | README, CLAUDE.md, changelog drafts | new topic branch | `README.md`, `docs/src/content/docs/**` (not `docs/public/wind-examples/`), `CLAUDE.md` | Lands after L1/L2 decisions for content | Separate current / opt-in / planned features; draft five-lane notes and a version-bump proposal from actual changes; no publication |

Shared-file rules: `Cargo.lock` is L1-only (a 4268 size probe may use a throwaway draft PR that
is never merged). `docs/public/wind-examples/manifest.json` is regenerated once by whichever lane
integrates Rust changes, never hand-edited. `crates/zfb/src/commands/dev.rs` belongs to L2 while
4307 is open, so 4133 work waits or coordinates.

### Native qualification status (4294)

The 2,094 "unrun" figure is superseded. The native first run at `21d6807e` executed 6,854 tests:
6,838 passed, 7 failed, 9 timed out, 0 unrun; doctests exit 0. Original JUnit is lost and is not
treated as recovered.

| Group | Tests | Later fixed-SHA evidence | Open |
| --- | --- | --- | --- |
| Graph identity (2 failed) | `build_package_routes::virtual_module_only_import_ships_as_island`, `scratch_dir_concurrent_e2e::concurrent_dev_build_check_are_isolated` | Product fix 4308; native PASS at `92e25ad4` (13 + 7 + 1 + 1) | No full-suite run at `21ef7c4` |
| Watcher (9 timed out at 300 s) | `zfb-watcher::recursive_dirs` (9 cases) | Test redesign 4309; native 23/23 at `a76e4775` | Root cause of the original stalls unexplained; original XML lost |
| Server watch (3 failed) | `zfb-server::watch_add_confirm` (3 `real_watcher_*`) | 4309; native 3/3 at `a76e4775` | Same |
| Dev bind (2 failed) | `dev_bind_before_walk_e2e` (2) | 4309; native 2/2 at `a76e4775` | Same; not run natively after `b605a4de`; no native run since `a76e4775` |

None of the 16 failures from `21d6807e` reproduced on current main; native Mac passes for them
were recorded at `92e25ad4`, `a76e4775` and `b605a4de`. The two pre-4321 Mac captures (26 tests:
23 + 3) used the old helper and are not evidence for the helper now on main. The new helper made a
real capture on Linux at `21ef7c4` (`recursive_dirs` 23/23); that is not Mac evidence. A separate,
limited native Mac receipt now exists at `21ef7c4`
([receipt](https://github.com/Takazudo/zudo-front-builder/pull/4322#issuecomment-6274889047)):
the new helper's 25 unique offline tests passed once each under logical and physical TMPDIR, and a
real capture of `zfb-graph --lib --locked --retries 0` passed 43/43 (cargo-nextest 0.9.140, `ci`
profile, isolated clean source/target), with JUnit byte identity, SHA-256 and receipt/source
fingerprints checked. Its scope is this SHA's helper and that graph lane only: it claims nothing
about the 4315 Darwin producer, V8, the watcher, the full workspace, b4push, live admission or release. 4305 merges
cleanly into main and its 20 offline harness tests pass, but the native sharded run has never been
dispatched; adoption waits on the owner (nextest is being unified on 0.9.140). 4133 (eight
`cargo test -p zfb --lib` Mac hangs) did not reproduce under nextest and remains unverified; 4309
does not cover it. Evidence links are in the latest comments on 4294, 4305 and 4133.

### Issue sweep classification (executed by L4; closures carry evidence comments)

- **Closed as completed (2026-10-19):** 4251 (via 4298 `15a76210`); 4219, 4218, 4224 and 4223
  (via 4234 `d28cdd57`); 4098 (4120 merged; remaining work tracked in 318, 4123, 4124, 4133).
- **Open, waiting on 4231:** 4231 stays open until 4328 merges (it adds the missing comparison of
  workspace-build and packed-tarball-build bytes); 4252 (S7 ledger), then 4243 and 4241, close after it.
- **B, superseded:** PR 4306 (contained in 4307).
- **C, Cloud work:** packed-build byte-comparison test for 4231: [4328](https://github.com/Takazudo/zudo-front-builder/pull/4328) head `732a03de`, CI 15 success / 0 failure, ready for review and a main candidate; 4231's acceptance criteria were replaced to match it; 4252 S7
  ledger; 4235 body refresh; 4019 (the `tests/process-reaping-diagnostic/diagnose.py` probe passed
  on the current Cloud init, but that is not the real supervisor suite, so it stays open); 4268
  size probe done: draft 4327 measured `render` at 1,114,879 B, over the 1,100,000 B ceiling;
  recorded on 4268 and the probe closed (tarball, npm test and browser legs NOT RUN; pin stays `=0.0.55`).
- **D, waits on Mac/env:** 4294, 4133, 4123, 4124, 4276, 4315/4319 Darwin collection.
- **E, owner decision / HOLD / trigger:** 4289 and PR 4293 (HOLD: drain contract); 4303 (default
  cutover and Deno removal gates); 4302 (after 4301); 4305 (adoption; nextest unified on 0.9.140); 4311
  (provenance policy); 4317 (merge decision); 4268 (0.0.58: 2.3–2.9x slower and `render` over ceiling);
  4304 (red only from intentional zudoDoc identity drift; await the 2026-10-25 watch); 1373
  (upstream triggers; the `deny.toml` bincode reason is current: the lock still shows `deno_core`
  0.399.0 and `syntect` 5.3.0 on bincode 1.x); 318, 3386, 3372, 3879, 3357; 1492 stays sticky.

## Historical checkpoint 4294 (integrated to main by PR 4312)

- Branch: `base/4294-development-checkpoint`; parent: `21d6807e442bbed61e578c165a91d64c4a7dc141`.
- Scope: reviewed graph-path fix from PR [4308](https://github.com/Takazudo/zudo-front-builder/pull/4308),
  bounded watcher/dev tests from PR [4309](https://github.com/Takazudo/zudo-front-builder/pull/4309),
  ICU publication fix from PR [4313](https://github.com/Takazudo/zudo-front-builder/pull/4313),
  qualification-evidence docs from PR [4316](https://github.com/Takazudo/zudo-front-builder/pull/4316),
  the per-command nextest evidence helper from PR [4318](https://github.com/Takazudo/zudo-front-builder/pull/4318),
  and this workflow policy. Native acceptance of the combined checkpoint is not claimed.
- Development head: resolve `git rev-parse origin/base/4294-development-checkpoint` after fetching.
  The aggregation PR records each immutable checkpoint SHA and exact-head CI receipt. A later
  tip does not inherit the earlier receipt. This document cannot contain its own commit hash.
- At intake all remote branch heads and all seven open PRs were inspected. Existing `base/**`
  heads concern 4187, 4199, design-system playground, and post-4.5.0 maintenance; none is adopted
  as an implicit parent. No active source branch is rewritten or closed.

| Source | Selected immutable head | Evidence at that head | Development decision |
| --- | --- | --- | --- |
| PR 4308 | `92e25ad4da392b0e566320f5e477e62bf134adc7` | [Linux health 39143914080](https://github.com/Takazudo/zudo-front-builder/actions/runs/39143914080) and other automatic workflows succeeded; preview upload skipped for draft | Integrate exact head; exact-source native follow-up passed (owner receipt below) |
| PR 4309 | `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a` | [Linux health 39146914767](https://github.com/Takazudo/zudo-front-builder/actions/runs/39146914767), 6,868 workspace tests passed; 27 checks succeeded, 6 conditional skips | Included; 28 selected native checks passed, with original watcher XML missing (receipt below) |

The source changes are disjoint except `docs/public/wind-examples/manifest.json`.
Each source PR generated its own compiler digest. The combined checkpoint must regenerate
with the official `docs/scripts/wind-preview-assets.mjs` and the matching built workspace
compiler, then run `--check`; neither parent's digest proves the combined sources.

### Historical local checkpoint evidence

The combined source tree was checked on Linux x86_64 with Rust/Cargo 1.99.1, Node 24.19.0,
repository-pinned pnpm 12.8.2, and SHA-verified esbuild 0.25.12. The policy Markdown and
`cargo fmt --all --check` passed. The 19 preview-generator contract tests passed. Focused Rust checks passed: 13 graph unit
plus 7 graph integration, 65 helper unit plus 11 helper integration, 23 recursive-directory
watcher and 3 server publication tests. One existing helper doc-test is explicitly ignored;
none of these executed tests failed or retried.
The official generator rebuilt all 173 previews with the combined-source CLI and `--check`
confirmed freshness; only the compiler digest changed, no sample record or generated CSS.
Compiler source digest: `dbdcdb240a9c3f22e8d3bc5062d515343259104f9d6c1829d8f4e8158738bf4b`.

The scoped `cargo build --locked -p zfb --no-default-features --bin zfb` passed under the
machine-wide heavy guard. It is a generation tool, not default-V8 acceptance. First attempts
remain distinct: an inaccessible guard directory caused fail-open behavior, so that process
was stopped during dependency setup (exit 143); the guarded build then failed on missing
locked `hono`, and after dependency installation on an esbuild downloader send error.
Installing frozen workspace dependencies and staging the exact hash-verified esbuild binary
resolved those prerequisites. No assertion failure was retried. The first commit-hook attempt
also rejected the host's pnpm 11.19.0; a command-scoped shim to the pinned Corepack pnpm fixed
it, with both hooks then passing. No global toolchain setting or hook bypass was used.

The subsequent integrated Linux batch is recorded in [PR 4312](https://github.com/Takazudo/zudo-front-builder/pull/4312).
At checkpoint `b605a4deb17c387f65cc2d8c81893473c84253b0`, tree
`6cd7a6da497ece0c09ffbc2d9c895ea24acfc2ce` matches the tested PR 4313 merge tree.
[Health 39168306313](https://github.com/Takazudo/zudo-front-builder/actions/runs/39168306313)
reported 6,874 workspace passes (86 excluded), 52 environment-gate passes (1,674 excluded),
and 457 md-extras passes: 7,383 executed JUnit cases. Doctests are separate.
This is recorded tree-equivalent evidence, not an all-green exact-head rollup: redundant
aggregation health/docs/binary runs were cancelled after the tree comparison. Cancelled jobs
remain cancelled. No local full-workspace run duplicates that batch.

### Trigger audit and external effects

Inspected workflow bodies at parent `21d6807e`; selected topic heads change no workflow.
Recheck this table at each checkpoint that changes workflows. Keep the aggregation PR **draft**.

| Route | Actual trigger/condition | Result for this checkpoint |
| --- | --- | --- |
| `node-free-smoke.yml` production showcase | Push to `main`; production job also requires `github.ref == 'refs/heads/main'` | No production job from the chosen branch/PR. Comments mentioning base pushes are stale; actual push list contains only main |
| `docs-deploy.yml` | Main push touching `docs/**` or its workflow, or explicit manual dispatch | No production dispatch; never push/merge this checkpoint to main |
| `docs-pr-preview.yml` | PR to main/base, docs-path filter; upload job requires non-draft | Docs verification/artifacts may run; preview upload skipped by existing draft guard |
| `node-free-smoke.yml` showcase preview | Same-repository, non-Dependabot, non-draft PR | Draft suppresses preview publication; builds/smokes/artifacts still run |
| `health.yml`, `docs-checks.yml`, `pr-checks.yml` | PR checks, including base targets where declared | One automatic Linux checkpoint batch; monitor exact head and PR merge SHA |
| `actionlint.yml`, `wind-computed-style.yml` | Path-filtered pushes/PRs including base branches | Existing validation may run; no deployment authority |
| `release.yml` and release-following smoke | Explicit release/manual event; `dry_run` defaults false; release smoke follows Release workflow | Not invoked. No release/tag/contract application in this lane |
| `rolldown-plugin-publication.yml` | Push to `prototype/318-rolldown-native` | Not the chosen branch; not invoked |

“Non-publishing” does not mean no external effects: GitHub CI consumes compute and uploads
logs/artifacts, and some checks may report issues. Converting this PR to ready can enable
preview publication on subsequent eligible events. No credentials, deployment conditions,
branch protection, skip markers or runner budgets are changed to suppress checks.

### Native qualification stays on selected source

Native verification is separately owned by the existing Mac qualification session under
[4294](https://github.com/Takazudo/zudo-front-builder/issues/4294). This manager does not operate
that machine, start a new full Mac run, or dispatch Mac Actions. The source-specific follow-ups
remain bound to the PR 4308/4309 heads above, unless that owner explicitly records a replacement.
The owner reports native PR 4308 verification complete at `92e25ad4da392b0e566320f5e477e62bf134adc7`:
13 graph unit tests, 7 integration tests, and both targeted CLI regressions passed; the corrected
virtual-module command actually executed one test (4.70s), without a skip. Independent review
found no material issues. The earlier compile timeout, host restart interruption and invalid-PATH
skip remain separate receipts, not clean passes. The [published receipt](https://github.com/Takazudo/zudo-front-builder/pull/4308#issuecomment-6254981484)
records Apple A18 Pro / ARM64, 8 GiB, macOS 27.0.1, Rust/Cargo 1.99.1, Node 24.14.0 and
hash-verified Darwin ARM64 esbuild 0.25.12. It retains full commands, binary hashes and log
checksums in the owner's local evidence archive; this lane has read the published receipt, not
those archive bytes. PR 4309 has its own [later native receipt](https://github.com/Takazudo/zudo-front-builder/pull/4309#issuecomment-6258920017)
at `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a`: 23 recursive_dirs, 3 watch_add_confirm and
2 named dev cases passed on their first execution, without retries. Compilation of three
binaries is compile-only evidence. One guard refusal preceded execution. The original watcher
JUnit XML was overwritten and is unrecoverable; its full log, 23 identities and exit remain,
while original server/dev XML survives. No reconstructed XML or replacement run closes that gap.
This supersedes earlier “native unrun” summaries only for those 28 selected checks; it neither
explains the original nine stalls nor qualifies the full suite or combined development SHA.
Do not silently retarget native work to this development branch.

| Qualification record | State and next action |
| --- | --- |
| Historical native first run | [Original report](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6251614904): 6,838 passed, 7 failed, 9 timed out, 86 excluded. Preserve raw receipt and first failures; not current-head acceptance |
| Graph follow-up | Owner reports PASS at frozen `92e25ad4…`: 13 unit + 7 integration + both targeted CLI cases. [Published receipt](https://github.com/Takazudo/zudo-front-builder/pull/4308#issuecomment-6254981484), with toolchain and local archive provenance; no integrated-SHA acceptance |
| Watcher/dev follow-up | 28 selected native passes at frozen `a76e4775…`; [receipt](https://github.com/Takazudo/zudo-front-builder/pull/4309#issuecomment-6258920017). Original watcher XML missing; full-suite qualification remains open. [Design](../crates/zfb-test-utils/timeout-test-design.md) |
| Mac build controls | Command-scoped `CARGO_BUILD_JOBS=1`, existing heavy guard, reusable caches; no global setting change or cargo clean. Separate compilation logs from test logs |
| Required receipt | Immutable SHA/tree; OS/CPU; exact rustc/cargo/Node/pnpm/esbuild/engine/ICU; target/profile/features/flags; command and cache identity; lock/input/binary hashes; artifact paths/digests; first exit/result and retries |
| Classification | Build timeout: incomplete build, tests not run. Behavioral failures/timeouts retain their distinct diagnostics. Skip, cancellation, pending or old-SHA success is never current acceptance |
| Release candidate | None selected or authorized by this checkpoint. Later approval freezes a SHA; only relevant reviewed fixes enter it, each with a new recorded SHA and affected requalification |

### Development and qualification matrix

These are development states at the snapshot, not release approval. None of the changes below
is claimed released. “Included” refers to the development checkpoint branch (heads `b605a4d` and
later), not to `main` before the checkpoint merge (`21d6807e`).
Exact source heads remain available through each linked PR; newer heads need new evidence.

| Work / immutable head | Implemented / checkpoint inclusion | Qualification and remaining boundary |
| --- | --- | --- |
| [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305), `bd02b8afaddf158cee76f857526d1fa1409c258c` | Native sharding harness; excluded | Linux harness checks; native sharded full run unrun; adoption separate |
| [4306](https://github.com/Takazudo/zudo-front-builder/pull/4306), `5162d753b1b771749a75c227ad91e653ac3045cd` | Error constructor fix; in 4307, excluded here | Isolated-head native gap is not filled by 4309; integration evidence belongs to 4307 |
| [4307](https://github.com/Takazudo/zudo-front-builder/pull/4307), `e467b9b49283e8177e8fb88053f10d22be4dc252` | Opt-in direct V8 integration; excluded | Linux native CLI/graph/hydration evidence in PR; real lock-admission failure blocks this chain; default Deno unchanged |
| [4308](https://github.com/Takazudo/zudo-front-builder/pull/4308), `92e25ad4da392b0e566320f5e477e62bf134adc7` | Graph-path fix; included | Source-specific native 13 unit + 7 integration + 2 CLI passes; receipt above; not combined-head acceptance |
| [4309](https://github.com/Takazudo/zudo-front-builder/pull/4309), `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a` | Bounded watcher/dev tests; included | Native subset 28 passes; missing original watcher XML and full-suite gap retained |
| [4310](https://github.com/Takazudo/zudo-front-builder/pull/4310), `7ca64ee35e663ee47b59b39796c3b2dcb90b289b` | External-edge admission capability; excluded | Linux capability checks; no live application approval or application |
| [4311](https://github.com/Takazudo/zudo-front-builder/pull/4311), `b596c4d009afe7b0a5d1445680d2ee57438e6261` | Owner-attested evidence capability, stacked on 4310; excluded | Linux checks only; integrity checks do not authenticate native execution; no native collection/application |
| [4312](https://github.com/Takazudo/zudo-front-builder/pull/4312), `b605a4deb17c387f65cc2d8c81893473c84253b0` | Development checkpoint | Tree-equivalent Linux evidence above; cancelled duplicate runs not passes; combined native qualification open |
| [4313](https://github.com/Takazudo/zudo-front-builder/pull/4313), `b605a4deb17c387f65cc2d8c81893473c84253b0` | Atomic ICU file publication; merged into checkpoint only | Offline publication checks and Linux batch; no native ICU execution claim |
| [4314](https://github.com/Takazudo/zudo-front-builder/issues/4314) / [4315](https://github.com/Takazudo/zudo-front-builder/pull/4315), `bda633b3589e201737b02cf2060f7b24d41a45a7` | Security issue / rustls fix candidate; excluded | Admission-blocked: live contracts retain old lock pin. Independent Linux checks passed; Cloud local compilation exhausted disk before tests; hosted Linux exact-source verification later passed (below). Native evidence/application remain separate |
| [4316](https://github.com/Takazudo/zudo-front-builder/pull/4316), `2b2d12d897c81791fa46d26032a667bde1d5e220` | Qualification/Cloud capacity docs; included by normal merge | Docs only; no execution claim |
| [4317](https://github.com/Takazudo/zudo-front-builder/pull/4317), `9e8fbd1487adc6cc922dfd5d4272fbf9e30ec82a` | Hosted Linux exact-source Rust lane, stacked on 4315; excluded | Executed once for source `bda633b` (below); security-branch evidence only |
| [4318](https://github.com/Takazudo/zudo-front-builder/pull/4318), `048a8f18d1b4d31a24c1760401994c3d875dac45` | Per-command nextest evidence helper; included by normal merge | 15 offline helper tests; owner-reported WSL real capture (below); no native Mac claim |
| [4319](https://github.com/Takazudo/zudo-front-builder/pull/4319), `02e25b55f4de76152af82483a79c8a6b95cdc599` | Local Darwin security-evidence adapter, stacked on 4315; excluded | Final-head CI passed; no real Darwin size collection or live application yet |

For 4315, the [capacity and check receipt](https://github.com/Takazudo/zudo-front-builder/pull/4315#issuecomment-6266482957)
records Clippy, 457 md-extras tests, 10 jsx-pragma tests, no-V8 compile and saved Linux artifact
replay passing. Doctests report 3 passed/7 ignored; compile-only examples are not execution.
In that Cloud environment, full nextest and scoped environment-gate builds failed before
execution: **zero tests executed** there; that first result is retained. Saved-byte replay is not
full original-input replay or Darwin measurement.

The Linux-only option was later implemented in draft (unmerged) PR 4317 and executed once on a standard hosted
Ubuntu 24.04 runner: [run 39248247275](https://github.com/Takazudo/zudo-front-builder/actions/runs/39248247275),
workflow/control head `9e8fbd1487adc6cc922dfd5d4272fbf9e30ec82a`, checked-out source
`bda633b3589e201737b02cf2060f7b24d41a45a7`, Rust 1.99.1, no retries. Every recorded check exited 0:
build, inventory, Clippy, workspace nextest **6,874 passed / 86 skipped**, doctests 3 passed /
7 ignored, md-extras 457, md-extras Clippy, no-V8, jsx-pragma no-V8 10 (986 filtered), and the
environment-gate filter 52 passed / 1,674 skipped. These are separate selections over overlapping
test sets; do not add them into a unique total. This is evidence for the security branch source
only, not for this checkpoint or `main`, and it neither evaluates nor applies live admission.

PR 4319 (head `02e25b55f4de76152af82483a79c8a6b95cdc599`, stacked on 4315) passed its final-head
CI: [local Darwin evidence 39254654825](https://github.com/Takazudo/zudo-front-builder/actions/runs/39254654825)
(synthetic evidence) and [binary/scaffold 39254654886](https://github.com/Takazudo/zudo-front-builder/actions/runs/39254654886).
Real Darwin size collection and live application have not happened. The security owner owns
4315/4317/4319 and admission; none of them is imported into this checkpoint.

### Per-command nextest evidence capture (PR 4318)

PR 4318's helper (`scripts/capture-nextest-evidence.py`) passed its 15 offline tests. The
[owner-reported WSL capture](https://github.com/Takazudo/zudo-front-builder/pull/4318#issuecomment-6269269513)
used source `b605a4deb17c387f65cc2d8c81893473c84253b0` and tool head
`048a8f18d1b4d31a24c1760401994c3d875dac45` with nextest 0.9.140, Python 3.12.3, Rust 1.95.0 on
WSL2 ext4/inotify: `recursive_dirs` 23 passed and `watch_add_confirm` 3 passed; child and wrapper
exits were 0 with `status=captured`. The first capture's evidence was byte-unchanged after the
second; the second `preexisting.xml` matched the first original XML, and the second report had a
new run UUID. This rests on the owner's report: the raw files stay on the owner's WSL host and
were not independently retrieved or re-executed from Cloud. It does not recover #4309's lost
native XML, establish FSEvents/Windows behavior, or qualify a Mac run.

### Holds, fixups and next topics

| Work | Owner / status | Next permitted action |
| --- | --- | --- |
| Combined checkpoint Linux evidence | Checkpoint manager | Integrated batch recorded above; keep tree-equivalent evidence distinct from cancelled duplicate jobs and later-head acceptance. Heads after `b605a4d` (4316/4318 merges) need their own exact-head CI, recorded in PR 4312; until then it is pending |
| PRs 4315/4317/4319 (rustls security chain) | Security owner | Excluded from this checkpoint. Real Darwin size collection, admission resolution and any live application remain open |
| Native failures from 4294 | Native qualification owner | Complete bounded source-specific follow-ups, preserving idle-host report and unresolved native stalls; Linux does not certify FSEvents |
| PR 4307 (`e467b9b49283e8177e8fb88053f10d22be4dc252` at intake) | Runtime integration owner; real Cargo admission unresolved | Excluded. Resolve admission and evidence before this affected chain proceeds; no live contract application |
| PR 4310 (`7ca64ee35e663ee47b59b39796c3b2dcb90b289b` at intake) | Separate admission/local-provenance task | Do not duplicate or integrate its follow-up here; implementation approval is not application approval |
| PR 4306 | Error identity lane, already part of separate 4307 work | Do not separately import into this checkpoint or confuse its native evidence with these topics |
| PR 4305 | Native Mac sharding proposal | Separate review/adoption decision; not imported. Four cold shards may compile four times; no assumed fourfold speedup |
| PR 4293 | HOLD, bounded-drain proposal | Unchanged, unimported; no implicit adoption |
| Next independent topic | Development manager | Select reviewed dependency-safe work against this base, with focused checks; no need to wait for unrelated Mac qualification |

Unrelated understood failures need a named fixup and retained evidence, not a project-wide
freeze. Security, data-loss, unknown foundation and contract-admission failures block their
affected dependency chain. No source PR is closed by this ledger and no runtime default changes.
