# Native documentation restructuring — #4079

Implementation scope: #4079; execution epic #4080 and work items #4081–#4085. This is a draft implementation for review. No merge, release, package publication, or deployment was performed or authorized. The seed issue remains open.

## Plan and implementation

1. Establish shared Architecture/Wind membership and explicit EN/JA route migrations before changing content or generation.
2. Fold Install into four flat numbered Getting Started chapters; move Concepts into Architecture, retaining all 28 articles under four native overviews.
3. Generate one complete compatibility page per locale, retain all utility family URLs and authored group pages, and update exact inventory/coverage contracts.
4. Compose the native Header and Sidebar factories over a scoped clone of the native tree; preserve the original route context and unknown-page fallback. Configure native homepage secondary links and wide/no-TOC Playground indexes.
5. Integrate content, run focused checks, then batch strict build, emitted HTML/link/anchor/island audits and actual-host browser acceptance. Leave unavailable-platform checks explicit.

The branch began at refreshed main `fd954c85d5ce04832f58e3b7fb83398a401d227d`. Independent Astra review approved the plan and final public-API composition. The user-authorized no-merge exception used disjoint child file ownership and manager-owned commits on one task branch. No prototype renderer was transplanted. #3329 remains excluded and is not a prerequisite.

PR #4075's Wind root-label/generated-preview changes were reviewed; they are on pending integration PR #4078 rather than this main base. This change does not edit the Rust implementation or generated preview manifest and does not reuse that branch.

## Native seams and preserved evidence

- Shared metadata owns four Architecture groups/28 articles, seven utility groups/48 families, eight Learn articles, four Wind entry areas, 23 compatibility buckets, and header order. Native index props are materialized with `node docs/scripts/sync-navigation-indexes.mjs`; `--check` fails on drift and missing/duplicate/unrecognized native components. Pinned zfb 2.20.2 drops named MDX imports, so raw named-import expressions were not retained.
- Public paired Header/Sidebar factories receive every slot prop. The cloned buildNavTree calls the original with all arguments and preserves native href/hasPage/metadata. It caches factories, not rendered trees. Missing known nodes fail closed; unknown pages use ordinary native navigation. Wind sidebarsConfig stays absent. Architecture physical positions also determine the tested native pager.
- Japanese homepage uses public `prepareHomeData` with the native default-only navigation filter disabled, then the unchanged native `HomePageView`; filtered locale tags/tagCount remain intact. This restores English Changelog/Claude compact links without changing default-locale-only URL policy or introducing a homepage renderer.
- All 55 pre-existing EN/JA Architecture article bodies were preserved after link rewrites (the dynamic-route illustrative URL and matching parameter were consistently renamed). Original standalone install/first-site fenced commands and heading anchors remain available.
- Compatibility retains all 1,288 complete eight-field rows per locale, byte-identical to the reviewed bucket row strings, and all 219 catalog entries. Source/profile/catalog pins, digests, row accounting, unsupported-mapping and false-browser-evidence negative controls remain active. Source-inspected evidence is not a browser or published-release claim.
- Fifty explicit route migrations yield 200 EN/JA slash/no-slash redirect rules. The 23 compatibility buckets map to stable canonical `#compat-*` sections. Historical route/anchor fixtures remain unchanged; audits resolve the explicit migrations.

## Verified revision and commands

Product revision: `cdad876e47e54564888f94f6806e83de8dff3383` (following initial implementation `7293ff28fcc7e22593fb9b60218441641194440d`). The subsequent evidence commit changes only the verification harness/report; the generated site and product source remain this revision.

Commands use the repository-pinned pnpm 12.8.2 through `npx --yes pnpm@12.8.2`; the host PATH pnpm 11.19 is incompatible. Commit formatting hooks ran successfully with pinned pnpm, not bypassed.

| Check | Result |
| --- | --- |
| `vp test run scripts/__tests__/{docs-navigation-structure,generate-wind-reference}.test.mjs` | PASS: 27 tests |
| `node --test docs/scripts/__tests__/audit-wind-{docs-coverage,built-anchors,built-links}.test.mjs` | PASS: 17 tests; coverage parses 177 sources |
| `node docs/scripts/generate-wind-reference.mjs --locale en --check` and `--locale ja --check` | PASS: 219 entries / 48 families; no generated drift |
| `node docs/scripts/sync-navigation-indexes.mjs --check` | PASS; fail-closed negative controls retained |
| `pnpm --filter docs check` | PASS: two collections and TypeScript |
| `pnpm --filter docs build` (`zfb build --strict-broken`) | PASS: 655 pages; search 647 entries; EN llms 463 / JA 184 pages |
| `node docs/scripts/audit-wind-built-anchors.mjs` | PASS: 116 Wind routes, 1,618 retained heading IDs across 2,082 IDs; 42 adjacent routes / 682 IDs |
| `node docs/scripts/audit-wind-built-links.mjs` | PASS: 18,822 local links from 158 captured routes, plus 4,646 inbound links |
| `html-validate 'dist/**/*.html'` from docs | PASS |
| `node scripts/assert-island-markers.mjs` from docs | PASS: 106 route/marker pairs and 10 island assets |

The final build/emitted-audit batch ran through the machine-wide heavy guard: `verdict=PASS`, 102 seconds, minimum memory 15,155 MB. Browser scripts also run through both heavy and Playwright guards against `wrangler dev --local --port 4333 --ip 127.0.0.1` (Wrangler 4.85.0). That Cloudflare local assets runtime parsed all 428 current redirect rules; checks exercise real HTTP redirect responses rather than emulating `_redirects`.

Browser scripts: `tests/docs-navigation-structure/{verify,content-verify,workshop-verify}.mjs`. All three pass on the same product revision. The first two passed in the consolidated run; the workshop follow-up passed after correcting its mobile test to open the native menu before using the Playground link. Probe corrections also normalize native leaf trailing slashes, exclude language-switch links from sidebar membership, wait for visible island mount, and await native `zfb:after-swap` before the next injected navigation link. These were harness errors, not weakened product requirements. No failing suite is labeled passed: only the corrected successful runs are recorded.

- Structure: all shared native cards resolve in EN/JA; exact three compact links; all 200 actual HTTP 301 targets and canonical anchor landings; direct/soft group navigation, active entries, desktop/mobile persisted drawer after multiple unopened navigations, and back/forward; Wind entry/Learn/compatibility/migration scopes.
- Responsive: 1440, 1280, 1024, 768, 390 and 360 px in both locales. All 23 compatibility anchors/tables and 1,288 rows exist, native table overflow works at narrow widths, no page overflow or TOC; Playground wide shell and workshop ordering remain intact.
- Content: eight native homepage blocks in exact order, four numbered chapter rows, npm/pnpm and Basic blog/Node-free tabs with complete commands; 66 native pager pages (root + four group overviews + 28 articles per locale); canonical search results and actual navigation for Installation/Routing/Grid; light/dark controls, theme persistence, language-link active state and English fallback inside the Japanese shell.
- Supplemental: all eight workshop/Wind scenarios pass (EN/JA × 1440/390). Workshop tests cover three presets, valid/invalid token edits, reset, three preview modes, exact export file bytes and downloaded ZIP bytes, starter demo form, default/light/dark host theme, and native leave/return state restoration with exactly one live island. Wind checks traverse every group's native index/family links, exact sidebar membership, native soft-navigation sentinel, and visible mounted preview code/viewport controls.
- Zero uncaught page errors. Screenshots were visually inspected for desktop first-track placement and mobile stacked controls; screenshot binaries remain local, not in Git. Compact structured evidence is committed alongside this ledger.

Workshop measurements: two tracks at 1440 (`280px 986px`), 1280 (`280px 838px`), 1024 (`250px 631.188px`), and 768 (`235px 483px`); controls precede preview in stacked 390/360 layouts. Direct children retain token-editor then preview-workspace. Workbench `::before` and `::after` content are `none` in all 12 geometry cases. No blank leading track or page-level overflow was observed. The existing responsive threshold was not changed.

## Limits and remaining work

- WebKit acceptance is **unrun** in this managed environment with known missing OS dependencies; tracked in #4087 (`deferred-verification`). No privileged provisioning, weakened assertion, disabled CI gate, or native/macOS gate removal was used. Chromium results do not imply WebKit results.
- The reported workshop controls-in-second-column state was not reproduced under the recorded native-host scenarios. No workshop CSS correction is claimed; original screenshots/state remain unavailable. The report remains unresolved for the original production conditions.
- Local Cloudflare assets runtime is an actual redirect-capable development host, not a production deployment. Production routing/CDN verification remains after authorized deployment. Doc-history generation uses the repository's normal local skip behavior.
- This configuration has no versioned docs and uses `/` as its base. Native helper calls and hrefs are preserved; alternative configured-version/base-path live builds were not exercised.
- Broad Rust/workspace and native platform CI remain their existing gates; no engine source or dependency versions changed. This documentation-focused run does not claim those suites passed.
