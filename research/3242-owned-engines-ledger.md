# Owned Engines decision, consumer, and test ledger

This ledger records the owner direction and implementation decisions for epic #3242, corrections to its design handoff, a re-measured in-repository inventory, and the test disposition map. Current-tree measurements were taken on 2026-09-28 from the assigned `base/owned-engines` tree at `704f42c87703e4f62eb4eeaf1758a0af557520f3`. Downstream figures are quoted from the dated consumer summary and were not re-measured here. The temporary `_temp-resource/3242-owned-engines/` directory is planning evidence; the facts needed by later tasks are written out below.

## Owner decisions

The first table preserves D01–D08 from the handoff's owner-approved decision table. The three following quotations are copied from the epic's “Owner direction” section.

| ID | Decision | Consequence |
| --- | --- | --- |
| D01 | The owner is zfb's only current user; the new engines target the owner's consumers. | Optimize the contract for the owner's applications and development workflow. |
| D02 | Dropping Tailwind compatibility is explicitly acceptable. | No promise to accept the whole Tailwind language, plugin system, default theme, or future changes. |
| D03 | Build `zudo-wind` with an independently fixed specification. | Adopt individual utilities/ideas on the owner's timing; version the owned behavior. |
| D04 | Dropping React/Preact compatibility is explicitly acceptable. | No required hooks, component class, context, VNode, or third-party component emulation. |
| D05 | JSX/component authoring and hydration are the essential runtime goals. | Keep composition, server HTML, and actual adoption of existing DOM. |
| D06 | A small built-in rendering model is preferred. | Favor explicit lifetimes and a bounded API over generalized extension points. |
| D07 | esbuild stays. | Do not migrate to Rolldown, replace the linker, or invent another JS/TS parser/compiler in this project. |
| D08 | Implementation continues with a local AI agent. | This package is a concrete handoff, not an implemented or deployed engine. |

> legacy removal is ok to be included. zudo-doc is my project. so I can handle. good news is that this lib is big but the user is only me. so KILL the backward compat thing aggressively. we should stay minimum

> we should have docs our JS/CSS way in our doc, because it's perfectly ours. needs explanation. maybe tailwind's forked guide too? we might need another zudo-doc base for it, or something like that

> we'll cut the new major version for this of course. then the backward engine should be killed

**Session direction recorded as facts:** the epic's branch starts from `main` and the root pull request targets `main` (epic section “Branches”); the implementation run is autonomous (epic section “Rules for the autonomous run”). Sub-issue branches merge into `base/owned-engines` before its root pull request.

## Delegated decisions

DD1–DD20 reproduce the planner's decisions and review targets from #3244, with the risk and mitigation column supplied by that issue.

| Id | Decision | Rejected alternative | Risk and mitigation | Review checks |
| --- | --- | --- | --- | --- |
| DD1 | Base branch `base/owned-engines` forks from `main`; the root pull request targets `main` | Nesting under the merged working branch | none | The base of the root pull request |
| DD2 | Legacy removal is in scope; the final state has no Tailwind, Preact or React code, config key, binary or CI lane | Coexistence on `main` | Large diff; staged inside the epic branch only | A search for `tailwind`, `preact`, `FrameworkKind`, `JsxRuntime` leaves only historical files and the frozen `docs/` host |
| DD3 | zudo-react ships inside `@takazudo/zfb` as subpath exports under `./zudo-react` (source `packages/zfb/src/zudo-react/*.ts`); a consumer's `jsxImportSource` is `@takazudo/zfb/zudo-react` | An eleventh lockstep package (sixth changelog lane, about twelve gate and release edit sites, first-publish setup); an injected alias fallback (duplicate-instance hazard) | Coupling to the SDK package; a later extraction is a move plus a re-export | Both exports maps; Scaffold E2E resolves it from a packed tarball; the embedded tree holds every entry file; one runtime instance per browser graph |
| DD4 | Build beside, cut over, delete, all on the epic branch. A temporary seam (wind selected by presence of the `wind` key; the value `framework: "zudo-react"`) exists only between the seam tasks and the cutover tasks | One atomic task flipping SDK, emitter, engine and about sixty fixtures | Seam code is throwaway | No seam symbol remains when the root pull request is opened for review |
| DD5 | New `wind` config key whose nested structs reject unknown fields; `wind: false` means authored CSS only; a leftover top-level `tailwind` or `framework` key is a hard error with a named migration message | Silent ignore | zudo-doc 5.x emits both keys, so it migrates before it bumps zfb, which is the intended order | Error text, docs page, both templates |
| DD6 | `zfb css` stays as the standalone wind compiler and its explicit source flags keep their meaning; `zfb wind explain` and `zfb wind audit` are added | Deleting `zfb css` | The zudo-doc package build depends on it | CLI help; goldens regenerated from wind |
| DD7 | The wind v1 language is ratified by a decision task from measured consumer tables, not from the handoff defaults | Adopting the handoff v1 verbatim | A wrong cascade or reset choice causes silent visual regressions | The spec records one disposition per feature with its evidence |
| DD8 | Runtime v1 includes whole-document server rendering, a trusted raw-HTML hatch (static and reactive), conditional regions, keyed lists, a scope-owned reactive effect, adapters for text, textarea, checkbox, select and radio, a non-JSX element constructor, and description introspection for `<Island>` | The handoff v1 (static composition plus scalar bindings) | A bigger runtime | Each feature traces to a measured call site |
| DD9 | New docs are sections of the existing docs site; the `docs/` host components stay on the published 2.x stack | A second docs site or a second zudo-doc base | Docs describe the new major from the moment the epic merges | Header nav, English and Japanese parity, strict build |
| DD10 | Guides are original prose; the Tailwind framework is MIT, but the text of its documentation site is not openly licensed, so structure may be similar and nothing is copied | Forking upstream docs pages | Licence exposure | No lifted passage; a licence notice only where upstream MIT source was copied |
| DD11 | `packages/zfb-ssr-identity-fixture` and the published-zudo-sg identity test are deleted | Keeping a Preact path alive for them | No coverage of a real published preset package until the next zudo-doc major | A follow-up issue filed by #3306 |
| DD12 | The release is not part of the epic; the owner cuts a breaking major after the merge; the epic leaves the breaking-change list ready | Releasing inside the chain | Docs lead the published binary until the release | Migration guide and the ledger list |
| DD13 | Heavy cargo work is serialized: one heavy lane and one light lane, encoded as resource edges in `Depends on:` | Up to six concurrent Rust worktrees | Long wall-clock time | The scheduler simulation in the epic |
| DD14 | `<Island>` keeps its wrapper element and the `data-zfb-island*` attributes; hydration markers exist only inside island subtrees; page-level output is marker-free | Comment-delimited island roots | Required smoke scripts assert exact substrings | Smoke scripts unchanged and green |
| DD15 | Private consumers appear in public artifacts by name and aggregate counts only; the name is already public in this repository | Publishing the inventory of their internals | none | Issue bodies and committed resources |
| DD16 | The md-wasm `compile()` function loses its `jsxRuntime` option; shipped-size tables are refreshed from CI-measured artifacts only | Keeping an option with one value | A public API break in the md-wasm lane | The procedure of `crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md` was followed |
| DD17 | Real Japanese IME verification cannot be automated; it is recorded as deferred human verification, never as passed | Counting synthetic events as proof | none | The completion report lists the manual IME pass separately |
| DD18 | Documentation is written from the owned specs before the engines exist, then reconciled against the merged implementation by #3309 | Writing docs only after the engines land, which serializes fourteen docs tasks behind the heavy lane | Docs lead the code for most of the epic | The list of corrected pages and remaining differences from #3309 |
| DD19 | Reference screenshots are release assets linked from issue comments; only computed-style snapshots and scripts are committed | Committing images that are deleted later and stay in the history of `main` | Asset links live outside git | No image under `_temp-resource/` |
| DD20 | The JSX prop dialect of the runtime is HTML-spelled by default (`class`, not `className`); #3247 makes the final call | Accepting React spellings for compatibility | Higher migration cost downstream (272 `className` sites measured in zudo-doc at planning time) | The prop-dialect section of the contract and the migration guide |

### Breaking changes recorded so far

These are derived only from DD2, DD5, DD6, DD11 and DD16 for the migration/release material owned by #3305.

- Tailwind, Preact and React support is removed, including their config keys, binaries and CI lanes. The new engines make no compatibility promise for the old languages or component models.
- Config moves to the `wind` key with strict nested validation. `wind: false` selects authored CSS only. Existing top-level `tailwind` and `framework` keys fail with a migration message.
- The standalone `zfb css` command remains, but compiles with zudo-wind; `zfb wind explain` and `zfb wind audit` are added.
- The published-consumer identity fixture and its current zudo-sg test are removed; a follow-up issue tracks the coverage gap.
- `@takazudo/zfb-md-wasm` `compile()` removes its `jsxRuntime` option. Callers still passing it receive the existing unknown-field rejection.

## Corrections to the design handoff

The first eight rows are the epic's “What exploration changed versus the handoff” table, including all seven assumptions. The remaining rows merge exploration corrections that change the contract, implementation order, test ownership, or packaging work. Evidence cells name files and symbols, without line references.

| Handoff statement | Measured fact | Consequence | Evidence file |
| --- | --- | --- | --- |
| Start with the docs site as first consumer | `docs/` builds with published `@takazudo/zfb` 2.20.2 and `@takazudo/zudo-doc` 5.27.0; it does not run the workspace binary. | Freeze the docs host on the 2.x stack during this epic; build the first real slice with the in-repo `basic-blog` template. | `docs/package.json`; `pnpm-workspace.yaml`; `.github/workflows/docs-checks.yml` job `Docs gate`; `crates/zfb/templates/basic-blog/` |
| The runtime integrates at the adapter seam | One SSR bundle has one JSX source and one `renderToString`; pages, layouts, MDX and SDK helpers are rendered by that bundle. | Runtime rendering covers whole documents, and trusted raw-HTML insertion is required. | `crates/zfb-build/src/bundler.rs` synthetic entry and `renderToString`; `crates/zfb-content/src/mdx_jsx_emit.rs`; `packages/zfb/src/content.ts` |
| Embedding makes the runtime resolvable | `embed_runtime` supplies a fallback tree when the project lacks `node_modules`; installed consumers resolve the published package exports. The embedded V8 host does not perform package resolution itself; esbuild resolves modules before the bundle runs in V8. | Ship zudo-react as `@takazudo/zfb` subpaths. Test the packed consumer path and the embedded fallback separately. | `crates/zfb/build.rs` `embed_runtime` and `copy_ts_src`; `crates/zfb/src/commands/bundler_input.rs`; `crates/zfb-build/src/bundler.rs` |
| Import and asset handling can be reused | Lightning CSS flattens `@import`, but package `url()` attribution on the current utility path depends on the Tailwind sourcemap; authored CSS does not provide those package companion assets. | Add attribution and companion emission on the Lightning CSS bundling path before deleting Tailwind's map. | `crates/zfb-css/src/url_attribution.rs` `attribute_and_emit_package_urls`; `crates/zfb-css/src/css_imports.rs` `bundle_authored_css`; `crates/zfb-css/src/engine.rs` `take_package_url_companions` |
| A source plan is to be introduced | The current engine ignores its source list and has no retained candidate index across CSS passes. Build and dev use Tailwind's ambient source detection; page/content edits, additions and removals do not reliably reach a deterministic incremental CSS candidate set. | Treat the source plan, candidate index and change-set channel as new dev-loop work. Exclude generated output and preserve ownership/refcount behavior for removed sources. | `crates/zfb-css/src/engine.rs` `TailwindSubprocessConfig`; `crates/zfb/src/commands/css.rs` explicit-source branch; `crates/zfb-build/src/orchestrator.rs` `plan_for_changes`; `crates/zfb/src/commands/dev.rs` `build_dev_css_and_publish_mirror_roots` |
| The v1 grammar suffices | The shipped scaffold uses `first:`, `group-hover:`, slash opacity, `space-y-*`, `divide-*`, `size-*`, `tabular-nums`, and arbitrary tracking values. Its stylesheet also depends on a project palette. Downstream CSS utilities are deliberately unlayered and need named spacing tokens, peer/group variants, pseudo-elements and opacity modifiers; zudo-doc uses no `dark:`. | Ratify the supported language, variants, token model, reset and cascade from the measured tables, not from the handoff's proposed grammar. See the inventory below for the complete scaffold token row set. | `crates/zfb/templates/basic-blog/pages/index.tsx`; `components/callout.tsx`; `components/theme-toggle.tsx`; `styles/global.css`; `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md` |
| Conditional regions, keyed lists, effects, select and radio can wait | In the dated zudo-doc inventory, 9 of 19 islands use conditional regions or keyed lists, and 31 of 52 effects rerun on state. zudo-doc also uses controlled selects and radio controls. In-repo non-docs islands do not exercise form controls, so that part of the runtime contract needs a synthetic specimen. | Include the measured structural and form features in the owned contract; label the docs host as frozen and keep Japanese IME verification deferred per DD17. | `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md`; `_temp-resource/3242-owned-engines/exploration/explore-consumers-in-repo.md`; #3242 DD8 and DD17 |
| `zfb css` is not mentioned | The standalone command exists, zudo-doc's package build calls it, and the CLI command always selects Tailwind even when `tailwind.enabled` is false. It has explicit `--source` flags and a Tailwind-directive diagnostic path. | Keep `zfb css` as the standalone wind compiler with its explicit-source semantics, and add the wind explain/audit commands. | `crates/zfb/src/commands/css.rs` `run_css` and `unresolved_tailwind_directives`; `_temp-resource/3242-owned-engines/exploration/census-tailwind-census.md` |
| Reuse the CSS parser for candidate collection | `crates/zfb-css/src/scanner.rs` discovers CSS Module imports; it does not scan utility candidates. `NativeRustEngine` is a not-implemented placeholder, not a native compiler to extend. | Build a dedicated complete-candidate extractor and parser. Do not count the CSS Module scanner or native-engine placeholder as existing utility support. | `crates/zfb-css/src/scanner.rs`; `crates/zfb-css/src/native_engine.rs` `NativeRustEngine` |
| `minimal-v1` reset is a safe default | The scaffold's authored `.prose` rules read 27 `var(--color-neutral-N)` references over 10 shades and rely on Tailwind preflight for border defaults, headings, lists and controls. zudo-doc imports full preflight and has package-defined CSS tokens. | Define the reset and tokens from the consumer tables; migrate authored CSS and browser-visible defaults deliberately. | `crates/zfb/templates/basic-blog/styles/global.css` `.prose`; `docs/src/styles/global.css`; `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md` |
| Utilities can live in a new named cascade layer | zudo-doc intentionally emits utility rules unlayered so they override layered authored rules. Its package CSS import order is theme, safelist, content, page-loading, features, then consumer overrides. | Keep the measured ordering contract or document a deliberate migration before changing it; test computed styles rather than only CSS text. | `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md`; `crates/zfb-css/src/pipeline.rs` `splice_framework_after_layer_prefix` |
| Utility candidates come only from class attributes | Required extractor cases include literal class maps, templates touching an interpolation, escaped HTML attributes, source-language HTML, and script-generated classes in downstream consumers. The tracked repo has class-map, interpolation and escaped-attribute examples but no tracked `.ts` inline script that contains a class candidate. | Implement and test each present source shape, report the inline-script case as absent in this repo, and preserve the documented downstream requirement without exposing a private consumer's identity. | `crates/zfb/templates/basic-blog/components/callout.tsx`; `docs/src/components/playground/option-row.tsx`; `crates/zfb/tests/css_command.rs`; `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md` |
| The JSX runtime can be chosen by changing one enum | There are five Rust enums (`config::Framework`, adapter `Framework`, `FrameworkKind`, `JsxRuntime`, and md-wasm `JsxRuntimeOption`), two TypeScript unions, and one `FrameworkAdapter` interface. `FrameworkKind::from_jsx_import_source` silently maps every value except `"react"` to Preact. | Remove all selector/conversion sites and reject an invalid runtime identity explicitly; do not leave the silent Preact fallback as a hidden third choice. | `crates/zfb/src/config.rs` `Framework`; `crates/zfb-render/src/adapters/mod.rs` `Framework`; `crates/zfb-islands/src/bundler.rs` `FrameworkKind::from_jsx_import_source`; `crates/zfb-render/src/swc_pipeline.rs` `JsxRuntime`; `crates/zfb-md-wasm/src/lib.rs` `JsxRuntimeOption`; `packages/zfb/src/config.ts`; `crates/zfb-md-wasm/npm/src/types.ts`; `packages/zfb-runtime/src/framework.ts` |
| SWC runs the production JSX transform | Production page TSX uses esbuild's automatic JSX transform from the synthetic tsconfig (`jsx: "react-jsx"`, `jsxImportSource`). The separate `SwcPipeline` API remains in zfb-render and md-wasm; the CLI's normal TSX bundle is driven by esbuild. No zfb esbuild call passes `--jsx-dev`. | Wire the owned runtime into esbuild's `jsxImportSource`; retain SWC's React-named transform feature as an implementation name, not as a React runtime promise. Do not add a dev JSX entry solely to satisfy a handoff assumption. | `crates/zfb-build/src/bundler.rs` `write_synthetic_tsconfig`; `crates/zfb-render/src/swc_pipeline.rs`; `crates/zfb-md-wasm/src/lib.rs` `compile`; `_temp-resource/3242-owned-engines/exploration/explore-render-framework.md` |
| The adapter's `hydrate` shim is the browser hydration path, and both bundle modes ship | `__zfb_internal_hydrate.jsx` is an SSR-bundle shim with no production browser reader. Production islands use generated shared-bundle mount glue; `bundle_per_island` and its source template are test-only/not production-wired. `Adapter::pre_render_setup` and the private `@zfb/islands-runtime` package also have no production callers. | Replace the live shared glue and remove dead adapter, per-island and alternate-hydration paths. Do not port their tests as product behavior. | `crates/zfb-render/src/adapters/preact.rs` `PREACT_HYDRATE_SHIM_SOURCE` and `pre_render_setup`; `crates/zfb-build/src/bundler.rs` `SHADOW_HYDRATE_FILENAME`; `crates/zfb-islands/src/esbuild.rs` `render_shared_bundle_entry_source`, `bundle_per_island`; `crates/zfb-islands/npm/`; `packages/zfb/src/runtime.ts` |
| JSX imports come only from `island.ts`, and MDX only needs an audit | Three SDK modules import `react/jsx-runtime`; two esbuild paths alias it to Preact today. The MDX emitter also hard-codes a `react/jsx-runtime` Fragment import, independently of SWC's generated imports. | Retarget the SDK factories and MDX emitter to the canonical zudo-react subpath in one seam; test that SSR and browser graphs resolve one runtime identity. | `packages/zfb/src/island.ts`; `packages/zfb/src/content.ts`; `packages/zfb-runtime/src/client-router-component.ts`; `crates/zfb-content/src/mdx_jsx_emit.rs`; `crates/zfb-build/src/bundler.rs`; `crates/zfb-islands/src/esbuild.rs` |
| An island only needs to receive a component and props | `<Island>` reads the child descriptor's `.type` and `.props` before the child runs. Persistence, page swaps and failed island activation already depend on wrapper attributes, marker identity and disposal order. | The runtime needs a non-JSX element constructor/description introspection, a stable island identity, explicit disposal without accidental DOM clearing, and a per-island error policy. Keep the wrapper and `data-zfb-island*` contract per DD14. | `packages/zfb/src/island.ts` `captureComponentName` and `captureSerializableProps`; `packages/zfb/src/runtime.ts`; `packages/zfb-runtime/src/client-router/swap-functions.ts`; `crates/zfb-islands/src/esbuild.rs` |
| All package files are embedded under a source-neutral runtime resolver | The embedded tree is written under `vendor/@takazudo`; `copy_ts_src` copies only `.ts` files and skips hidden/`__` paths. Runtime resolution uses esbuild before the bundle reaches V8. A standalone package that is public would also require both dist-tag lists and a changelog lane. | Keep the chosen `@takazudo/zfb` subpath layout, verify every entry file survives embedding, and keep browser/server package identity consistent. | `crates/zfb/build.rs` `embed_runtime` and `copy_ts_src`; `crates/zfb/src/commands/bundler_input.rs`; `scripts/retire-next-dist-tag.mjs`; `scripts/advance-latest-dist-tag.sh`; root `CLAUDE.md` |
| Remove Tailwind by deleting the executable and its version constant | Tailwind is coupled to a compile-time digest environment stamp, CLI version output, embedded binary slots, toolchain pins, fetch/b4push scripts, CI cache keys, provisioning assertions and ignored-test lanes. | Remove the whole packaging and gate chain together with test-manifest updates; a dead binary slot or stale assertion still breaks required checks. | `crates/zfb/build.rs`; `crates/zfb/src/commands/css_support.rs` `ZFB_EMBEDDED_TAILWIND_SHA256`; `crates/zfb/src/cli.rs`; `crates/zfb-toolchain-pins/src/lib.rs`; `.github/workflows/health.yml`; `.github/workflows/exam.yml`; `.github/workflows/node-free-smoke.yml`; `scripts/fetch-tailwind.mjs`; `scripts/run-b4push.sh`; `tests/unit/release-binary-slot-restore.sh` |
| Test deletions need only update test files | Ignored tests are tied to exact health/exam scopes and nextest groups; the repository manifest requires the test row and scheduled home to move with each reclassification. | Change `crates/CLAUDE.md`, health/exam scopes and `.config/nextest.toml` in the same owning task; preserve the repo's parity gate. | `crates/CLAUDE.md`; `.config/nextest.toml`; `.github/workflows/health.yml`; `.github/workflows/exam.yml`; `scripts/check-exam-ignore-parity.sh` |
| Template edits are isolated examples | Templates are compile-time embedded and the basic-blog scaffold is exercised by required scaffold/build checks and the deployed showcase. | Coordinate engine capability and template CSS/runtime migration before changing the shipped scaffold; avoid leaving a period where the required template cannot build. | `crates/zfb/src/commands/new.rs` `TEMPLATES`; `.github/workflows/node-free-smoke.yml`; `crates/zfb/tests/end_to_end_basic_blog_build.rs`; `crates/zfb/templates/basic-blog/` |

## In-repo consumer inventory (re-measured)

### Basic-blog class candidates

These measurements apply to tracked files under `crates/zfb/templates/basic-blog/`. Tier A extracts string and template literals attached to `class=`, `className=`, or a `class:` key. Tier B includes another quoted literal only when at least half of its whitespace-separated tokens match the utility-shaped heuristic in the command below. Template expressions are replaced with a separator so adjacent source text stays visible. This is a regex inventory, not an AST scan or a claim about runtime-generated values.

- **127 distinct class tokens; 240 token occurrences.** Command: the Python scanner in this section prints the token list and both totals from `git ls-files`.
- **Distinct utility tokens by family:** antialiased 1; background 14; block 1; border 13; cursor 1; divide 3; flex 4; font 3; gap 6; grow 1; inline-block 1; items 1; justify 2; leading 1; max-width 1; min-height 1; margin-top 9; margin-x-auto 1; margin-y 1; padding-bottom 1; padding-top 1; padding-x 4; padding-y 7; rounded 2; scroll-margin-top 1; shrink 1; size 1; space-y 3; tabular-nums 1; text 31; tracking 3; transition 1; underline 1; uppercase 1; width 1. Command: same scanner; counts are distinct tokens after variants are removed.
- **Tokens per variant:** dark 22; small breakpoint `sm` 6; hover 5; first 1; group-hover 1. Stacked chains count once under each variant in the chain. Command: same scanner; it splits colons outside brackets and counts distinct full tokens.
- **Arbitrary values:** 2 distinct tokens, 4 occurrences (`tracking-[0.08em]` and `tracking-[0.2em]`). **Slash-opacity:** 5 distinct tokens, 5 occurrences. **Child-combinator utilities:** 6 distinct tokens, 6 occurrences (`space-y-*`, `divide-*`). **Non-utility classes:** 2 (`group`, `prose`), 2 occurrences. Command: same scanner.

Full token list used as the #3246 migration-table row set:

```text
antialiased bg-amber-50 bg-emerald-50 bg-neutral-100 bg-rose-50 bg-sky-50 bg-violet-50 bg-white block border border-amber-500 border-b border-emerald-500 border-l-4 border-neutral-200 border-rose-500 border-sky-500 border-t border-violet-500 cursor-pointer dark:bg-amber-950/40 dark:bg-emerald-950/40 dark:bg-neutral-900 dark:bg-neutral-950 dark:bg-rose-950/40 dark:bg-sky-950/40 dark:bg-violet-950/40 dark:border-neutral-800 dark:divide-neutral-800 dark:hover:border-neutral-700 dark:hover:text-neutral-100 dark:text-amber-300 dark:text-emerald-300 dark:text-neutral-100 dark:text-neutral-300 dark:text-neutral-400 dark:text-neutral-50 dark:text-neutral-500 dark:text-neutral-600 dark:text-rose-300 dark:text-sky-300 dark:text-violet-300 divide-neutral-200 divide-y first:pt-0 flex flex-col flex-wrap font-medium font-mono font-semibold gap-2 gap-4 gap-5 gap-x-3 gap-y-2 group group-hover:text-accent grow hover:border-neutral-300 hover:text-neutral-900 hover:underline inline-block items-center justify-between justify-center leading-relaxed max-w-2xl min-h-screen mt-1 mt-12 mt-14 mt-2 mt-3 mt-4 mt-6 mt-8 mx-auto my-6 pb-6 prose px-2 px-4 px-5 py-0.5 py-12 py-16 py-2 py-3 py-5 py-6 rounded-full rounded-md scroll-mt-8 size-8 sm:flex sm:gap-6 sm:mt-0 sm:px-6 sm:shrink-0 sm:w-48 space-y-2 space-y-3 space-y-4 tabular-nums text-2xl text-3xl text-accent text-amber-700 text-center text-emerald-700 text-lg text-neutral-400 text-neutral-500 text-neutral-600 text-neutral-700 text-neutral-900 text-rose-700 text-sky-700 text-sm text-violet-700 text-xs tracking-[0.08em] tracking-[0.2em] tracking-tight transition-colors uppercase
```

Reproduction command for the complete token list and counts:

```bash
python3 - <<'PY'
from pathlib import Path
import subprocess, re, collections
files=subprocess.check_output(['git','ls-files','-z']).decode().split('\0')
files=[p for p in files if p.startswith('crates/zfb/templates/basic-blog/') and p.endswith(('.ts','.tsx','.jsx','.js','.mjs','.md','.mdx','.html'))]
def literals(source):
 i=0; n=len(source)
 while i<n:
  if source.startswith('//',i):
   j=source.find('\n',i+2); i=n if j<0 else j+1; continue
  if source.startswith('/*',i):
   j=source.find('*/',i+2); i=n if j<0 else j+2; continue
  q=source[i]
  if q=="'" and i>0 and i+1<n and source[i-1].isalnum() and source[i+1].isalnum(): i+=1; continue
  if q not in '\"\'`': i+=1; continue
  start=i; i+=1; body=[]
  while i<n:
   c=source[i]
   if c=='\\' and i+1<n: body.extend((c,source[i+1])); i+=2; continue
   if c==q: i+=1; break
   body.append(c); i+=1
  value=''.join(body)
  if q=='`':
   out=[]; j=0
   while j<len(value):
    if value.startswith('${',j):
     j+=2; depth=1; quote=None; esc=False
     while j<len(value) and depth:
      c=value[j]
      if esc: esc=False
      elif c=='\\': esc=True
      elif quote:
       if c==quote: quote=None
      elif c in '\"\'`': quote=c
      elif c=='{': depth+=1
      elif c=='}': depth-=1
      j+=1
     out.append(' ')
    else: out.append(value[j]); j+=1
   value=''.join(out)
  yield start,i,value,source[max(0,start-48):start]
exact={'block','inline','inline-block','flex','inline-flex','grid','hidden','grow','shrink','antialiased','uppercase','lowercase','capitalize','normal-case','italic','not-italic','underline','overline','line-through','no-underline','tabular-nums'}
shape=re.compile(r'^(?:-?(?:bg|border|rounded|text|font|tracking|leading|gap|gap-x|gap-y|space-y|divide|flex|items|justify|cursor|size|w|min-w|max-w|h|min-h|max-h|p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|scroll-mt|transition|hover|dark|first|sm)(?:-|/|\[)|(?:block|inline-block|flex|grid|hidden|grow|shrink|antialiased|uppercase|tabular-nums))')
def parts(token):
 out=[]; start=0; square=paren=0
 for i,c in enumerate(token):
  if c=='[': square+=1
  elif c==']' and square: square-=1
  elif c=='(': paren+=1
  elif c==')' and paren: paren-=1
  elif c==':' and not square and not paren: out.append(token[start:i]); start=i+1
 out.append(token[start:]); return out
counts=collections.Counter()
for path in files:
 source=Path(path).read_text(errors='replace'); found=[]; ls=list(literals(source))
 for start,end,value,prefix in ls:
  if re.search(r'\b(?:class|className)\s*=\s*(?:\{\s*)?$',prefix) or re.search(r'\bclass\s*:\s*$',prefix): found.append(value)
 for start,end,value,prefix in ls:
  if re.search(r'\b(?:class|className)\s*=\s*(?:\{\s*)?$',prefix) or re.search(r'\bclass\s*:\s*$',prefix): continue
  if '<' in value or '>' in value or '=' in value: continue
  words=re.findall(r'[^\s]+',value.strip())
  if words and sum((parts(t)[-1] in exact or bool(shape.match(parts(t)[-1])) for t in words))*2>=len(words): found.append(value)
 for value in found:
  for token in re.findall(r'[^\s]+',value.strip()):
   token=token.strip('\"\'`,;{}()')
   if token and '${' not in token: counts[token]+=1
print('distinct',len(counts),'occurrences',sum(counts.values()))
print(' '.join(sorted(counts)))
families=collections.Counter(); variants=collections.defaultdict(set)
for token,n in counts.items():
 components=parts(token); base=components[-1]
 for variant in components[:-1]: variants[variant].add(token)
 if base in {'group','prose'}: continue
 prefixes=['scroll-mt-','space-y-','divide-','min-h-','max-w-','text-','font-','tracking-','leading-','rounded-','rounded','border','bg-','cursor-','size-','shrink','grow','flex-','flex','inline-block','block','gap-','items-','justify-','mt-','my-','pt-','pb-','px-','py-','w-','uppercase','tabular-nums','transition-','underline']
 family=next((p.rstrip('-') for p in prefixes if base.startswith(p)),base)
 families[family]+=1
arbitrary=[t for t in counts if '[' in parts(t)[-1]]
opacity=[t for t in counts if re.search(r'/\d+(?:\b|\])',parts(t)[-1])]
child=[t for t in counts if parts(t)[-1].startswith(('space-y-','divide-'))]
nonutility=[t for t in counts if parts(t)[-1] in {'group','prose'}]
print('families',dict(sorted(families.items())))
print('variant_distinct',dict(sorted((k,len(v)) for k,v in variants.items())))
print('arbitrary',len(arbitrary),sum(counts[t] for t in arbitrary),'opacity',len(opacity),sum(counts[t] for t in opacity),'child',len(child),sum(counts[t] for t in child),'nonutility',len(nonutility),sum(counts[t] for t in nonutility))
PY
```

### Extractor-source examples

The commands below examine tracked files only. Each example is a concrete source shape for #3253; a missing case is labeled explicitly.

| Case | Tracked example |
| --- | --- |
| Literal class map | `crates/zfb/templates/basic-blog/components/callout.tsx` (`SPECS` full `tone` and `accent` strings) |
| Template-literal candidate touches an interpolation | `docs/src/components/playground/option-row.tsx` (`last:border-b-0${...}`) |
| Escaped attribute text | `crates/zfb/tests/css_command.rs` (`<div class=\"bg-[#cc44dd]\">`) |
| String literal in an inline script in a `.ts` file | none in this repository. `packages/zfb-runtime/src/client-router/router.ts` has a `.className` literal and a separate module-script URL, but no class candidate inside an inline script. |
| `.mdx` source | `crates/zfb/templates/basic-blog/content/blog/hello-zfb.mdx` |
| `.md` source | `crates/zfb/templates/basic-blog/content/blog/styling-with-tailwind.md` |
| `.html` source | `tests/router-chromium/fixture/index.html` |

Commands used: `rg -n 'class|className' crates/zfb/templates/basic-blog/components/callout.tsx docs/src/components/playground/option-row.tsx`; `rg -n -F 'class=\\"' crates/zfb/tests/css_command.rs`; `git grep -n -E '<script|classList|className|class=' -- '*.ts'`; `git ls-files '*.mdx' '*.md' '*.html'` followed by a file-content search for class attributes. The script-literal command produced no tracked `.ts` inline-script class candidate.

### Authored CSS, config files, and inline test configs

- `crates/zfb/templates/basic-blog/styles/global.css` uses `@import "tailwindcss"` once, `@custom-variant` once, `@theme` once, and `@layer` twice (`base` and `components`). It has no `@source`, `@apply`, `@utility`, `@variant`, `@plugin`, `@config`, or `@reference` directive. Command: `rg -o '@(import|custom-variant|theme|layer|source|apply|utility|variant|plugin|config|reference|tailwind)\b' crates/zfb/templates/basic-blog/styles/global.css | sort | uniq -c`.
- The stylesheet has **27** `var(--color-neutral-N)` references over **10** shades: 50:1, 100:2, 200:5, 300:2, 400:3, 500:2, 600:2, 700:2, 800:6, 900:2. Command: `rg -o 'var\(--color-neutral-[0-9]+' crates/zfb/templates/basic-blog/styles/global.css | sed -E 's/.*neutral-([0-9]+)/\1/' | sort | uniq -c`.
- There are **41** tracked `zfb.config.*` files. **39** set `framework: "preact"`, **0** set `framework: "react"`, and **2** have no framework key (`docs/zfb.config.ts`, `crates/zfb/tests/fixtures/css-highlight/zfb.config.json`). **7** config files have a `tailwind` key. Reproduction commands from the repository root:

  ```sh
  git ls-files | grep -E '(^|/)zfb[.]config[^/]*[.](ts|json|mjs|js)$' | wc -l
  git ls-files | grep -E '(^|/)zfb[.]config[^/]*[.](ts|json|mjs|js)$' | xargs grep -lE '"framework"[[:space:]]*:[[:space:]]*"preact"|framework[[:space:]]*:[[:space:]]*"preact"' | wc -l
  git ls-files | grep -E '(^|/)zfb[.]config[^/]*[.](ts|json|mjs|js)$' | xargs grep -lE '"framework"[[:space:]]*:[[:space:]]*"react"|framework[[:space:]]*:[[:space:]]*"react"' | wc -l
  git ls-files | grep -E '(^|/)zfb[.]config[^/]*[.](ts|json|mjs|js)$' | xargs grep -lE '"tailwind"[[:space:]]*:' | wc -l
  ```

- For inline Rust test inputs, the exact serialized-config pattern finds **56** physical source lines in **19** `.rs` files, all under `crates/zfb/tests`. Raw Rust strings that contain JSON on multiple source lines are included when the key and value share a source line. Three more `.rs` files contain **4** serialized JSON lines inside ordinary Rust strings with escaped quotes. One additional test passes `framework: Framework::Preact` as a typed input in `crates/zfb/tests/framework_packages_no_pnpm.rs`; this is a test input, not serialized project config. Together these three shapes cover **61** config/input lines in **23** test files.

  The issue's planning count of **72 lines / 32 `.rs` files** across `crates/` is not reproducible as a config-only count from this tree. A broader same-line text search gives **62 / 23** under `crates/zfb/tests` and **78 / 34** under all `crates/`; the test-only result contains two non-config comment matches (`build_terminates.rs` and `workerd_parity_e2e.rs`), and the all-crates result also matches code and prose that merely mention both words. This likely reflects a broader textual pattern or a different planning snapshot; the issue does not provide its exact original command. A newline-aware scan finds the same **56** unescaped JSON occurrences and no JSON key/value pair split across physical source lines. These searches are limited to tracked Rust source under `crates/`, double-quoted JSON key/value syntax, the listed escaped-string form, and one explicit typed test input. They do not identify generated configs, runtime-built objects, alternate key/value spellings, or arbitrary strings constructed through interpolation. The shell smoke config is counted separately.

  ```sh
  rg -n --glob '*.rs' '"framework"[[:space:]]*:[[:space:]]*"preact"' crates | wc -l  # 56 physical lines
  rg -l --glob '*.rs' '"framework"[[:space:]]*:[[:space:]]*"preact"' crates | wc -l  # 19 files
  rg -n --glob '*.rs' '\\"framework\\"[[:space:]]*:[[:space:]]*\\"preact\\"' crates | wc -l  # 4 escaped lines
  rg -l --glob '*.rs' '\\"framework\\"[[:space:]]*:[[:space:]]*\\"preact\\"' crates | wc -l  # 3 files
  rg -n --glob '*.rs' 'framework[[:space:]]*:[[:space:]]*Framework::Preact' crates/zfb/tests/framework_packages_no_pnpm.rs | wc -l  # 1 typed input
  rg -n --glob '*.rs' 'framework.*preact' crates/zfb/tests | wc -l  # broader text: 62 lines
  rg -l --glob '*.rs' 'framework.*preact' crates/zfb/tests | wc -l  # broader text: 23 files
  rg -n --glob '*.rs' 'framework.*preact' crates | wc -l  # broader text: 78 lines
  rg -l --glob '*.rs' 'framework.*preact' crates | wc -l  # broader text: 34 files
  rg -n 'framework[[:space:]]*:[[:space:]]*"preact"' tests/smoke/node-free-ts/run.sh | wc -l  # 1 heredoc config
  ```

  The `rg -n | wc -l` forms count matching physical lines; the `rg -l | wc -l` counterparts count distinct files. The broad same-line command is deliberately not treated as a config count: it includes comments and typed code. Reproduce the newline-aware check and the 61-line/23-file union with:

  ```sh
  python3 - <<'PY'
  import re, subprocess
  from pathlib import Path

  paths = subprocess.check_output(["git", "ls-files", "-z", "--", "*.rs"]).decode().split("\0")
  patterns = {
      "plain JSON": re.compile(r'"framework"\s*:\s*"preact"'),
      "escaped JSON": re.compile(r'\\"framework\\"\s*:\s*\\"preact\\"'),
  }
  matched = set()
  total = 0
  cross_line = 0
  for path in filter(None, paths):
      text = Path(path).read_text(errors="replace")
      for pattern in patterns.values():
          for match in pattern.finditer(text):
              total += 1
              matched.add(path)
              if "\n" in match.group():
                  cross_line += 1
                  print("cross-line pair:", path, text.count("\n", 0, match.start()) + 1)
  typed = Path("crates/zfb/tests/framework_packages_no_pnpm.rs").read_text()
  typed_count = len(re.findall(r"framework\s*:\s*Framework::Preact", typed))
  print(f"serialized JSON pairs: {total}; files: {len(matched)}; cross-line pairs: {cross_line}")
  print(f"typed test inputs: {typed_count}; combined lines: {total + typed_count}; combined files: {len(matched | {'crates/zfb/tests/framework_packages_no_pnpm.rs'})}")
  PY
  ```

  The node-free TypeScript smoke script contains **one** `framework: "preact"` config in a heredoc. The exact JSON search misses escaped Rust-string syntax, comments, typed enum assignments, generated configs and runtime-built objects; broader textual matches can include unrelated code and prose.

### Client islands and hook call sites

A file counts when its first statement, after a UTF-8 BOM, whitespace and leading comments/reference directives, is the `"use client"` directive. Hook counts below are regex call-site counts (`\buse(State|Effect|Ref|Memo|Callback)\b\s*[<(]`), not AST counts.

- **15** tracked directive files: **11** engine-owned files and **4** docs-host files. Command: the Python scan below reads `git ls-files`, strips only leading comments, then tests the first statement.
- Engine-owned files have **15** hook call sites: `useState` 13, `useEffect` 2, `useRef` 0, `useMemo` 0, `useCallback` 0. The **11** files are: `crates/zfb-islands/fixtures/pnpm-workspace-consumer/workspace/zfb-blog-islands/src/index.tsx` (state 1); `crates/zfb-islands/fixtures/two-islands/components/counter.tsx` (state 1); `crates/zfb-islands/fixtures/two-islands/components/theme-toggle.tsx` (state 1); `crates/zfb/templates/basic-blog/components/theme-toggle.tsx` (state 1, effect 2); `crates/zfb/tests/fixtures/client-bundling-cross-pipeline/components/cross-pipeline/Island.tsx` (no hooks); `crates/zfb/tests/fixtures/client-bundling-cross-pipeline/sub-packages/reroot-host/src/RerootIsland.tsx` (no hooks); `crates/zfb/tests/fixtures/dev-islands-chunk-retention/components/probe-island.tsx` (no hooks); `crates/zfb/tests/fixtures/dev-islands-entry-probe/components/probe-island.tsx` (no hooks); `tests/built-site-smoke/fixture-site/components/counter.tsx` (state 1); `tests/built-site-smoke/fixture-site/components/gallery.tsx` (state 1); and `tests/md-wasm-browser-smoke/fixture-site/components/md-wasm-highlighter.tsx` (state 7). Command: the same scan plus the hook regex above, split by whether the path begins `docs/`.
- Frozen docs-host files have **46** hook call sites: `useState` 38, `useEffect` 2, `useRef` 4, `useMemo` 2, `useCallback` 0. The **4** files are `docs/src/components/playground/compile-playground.tsx` (state 10, memo 1), `highlight-playground.tsx` (state 7, effect 1, ref 2), `parse-playground.tsx` (state 9, memo 1), and `render-playground.tsx` (state 12, effect 1, ref 2). Command: same scan and call-site regex, restricted to `docs/`.

Reproduction command for the file split and hook totals:

```bash
python3 - <<'PY'
from pathlib import Path
import subprocess, re
paths=subprocess.check_output(['git','ls-files','-z']).decode().split('\0')
hooks=['useState','useEffect','useRef','useMemo','useCallback']
found=[]
for path in (p for p in paths if p.endswith(('.tsx','.jsx','.ts','.js','.mjs'))):
 text=Path(path).read_text(errors='replace')
 rest=text.lstrip('\ufeff \t\r\n')
 while True:
  m=re.match(r'(?s)(?://[^\n]*\n|/\*.*?\*/|<!--[\s\S]*?-->)(\s*)',rest)
  if not m: break
  rest=rest[m.end():]
 if re.match(r'^["\']use client["\']\s*;?',rest):
  counts={h:len(re.findall(r'\b'+h+r'\b\s*[<(]',text)) for h in hooks}
  found.append((path,counts))
for name,rows in [('all',found),('engine',[r for r in found if not r[0].startswith('docs/')]),('docs',[r for r in found if r[0].startswith('docs/')])]:
 print(name,'files',len(rows),'hooks',{h:sum(c[h] for _,c in rows) for h in hooks})
for path,counts in found: print(path,counts)
PY
```

### Raw-HTML insertion sites

The required `git grep -c` scan matches **6 lines in 5 tracked JS/TS files** outside `docs/src/content/`, and **30 lines in 7 tracked Rust files**. Commands from the repository root:

```sh
git grep -c -e 'dangerouslySetInnerHTML' -- '*.ts' '*.tsx' '*.jsx' '*.js' '*.mjs' | grep -v '^docs/src/content/' | awk -F: '{ lines += $NF; files += 1 } END { print lines, files }'
git grep -c -e 'dangerouslySetInnerHTML' -- '*.rs' | awk -F: '{ lines += $NF; files += 1 } END { print lines, files }'
```

Live insertion sites and data type:

| Site | Data | Classification |
| --- | --- | --- |
| `crates/zfb/templates/basic-blog/layouts/default.tsx` | `THEME_BOOTSTRAP_SCRIPT` | Static script text emitted by the server. |
| `docs/src/components/playground/highlight-playground.tsx` | `result.html` | State-driven preview HTML; docs host remains frozen on the 2.x stack. |
| `packages/zfb-runtime/src/client-router-component.ts` | `announcerCss` passed to a style vnode | Static CSS string. |
| `tests/md-wasm-browser-smoke/fixture-site/components/md-wasm-highlighter.tsx` | `html` | State-driven highlighted markup. |
| `crates/zfb-content/src/mdx_jsx_emit.rs` | HTML/MDX and syntax-highlight output wrapped in generated JSX | Authored-content-derived HTML, fixed for each render; not client state. |

The remaining grep hits are comments, Rust test strings/assertions, fixture data or API documentation: `crates/zfb-md-wasm/npm/test/api.test.ts` (one test comment), the comment line in `packages/zfb-runtime/src/client-router-component.ts`, `crates/zfb-build/src/bundler.rs`, `crates/zfb-content/src/pipeline.rs`, `crates/zfb-content/tests/mdx_jsx_emit.rs`, `crates/zfb-content/tests/mdx_jsx_emit_hast.rs`, `crates/zfb-md-ast/src/lib.rs`, and `crates/zfb/tests/html_minify_build.rs`. The `git grep -c` counts include these matches, so they are not additional runtime sites.

### Downstream evidence, quoted but not re-measured

The only public downstream consumer names recorded here are **zudo-doc** and **zudo-sg**. The dated read-only summary inspected zudo-doc at 5.25.0 and zudo-sg at 0.1.1; the in-repo docs host pins published zudo-doc 5.27.0. These are the summary's revision-specific figures, not measurements of current releases: roughly 550 distinct utility tokens for zudo-doc and 450 for zudo-sg; custom named spacing tokens are used in place of a bare spacing scale; neither uses `dark:`; zudo-doc's CSS contract requires unlayered utilities and an ordered package stylesheet import chain. zudo-doc has 31 of 52 effects that rerun on state and 9 of 19 islands requiring conditional or keyed structures. The source is `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md`.

A private consumer was also summarized there. Only aggregate facts are recorded: its candidate corpus is about three times the zudo-doc corpus, and it uses `@apply`, `@utility`, context and portals. No private consumer identity, paths or component inventory are copied here.

## Legacy-test disposition manifest

Rows are keyed by Rust test path plus test function, or by one test-suite file where the census names a suite. No line numbers are included. Every row has one disposition (`keep`, `port` or `delete`) and an owner. The Tailwind env-gate count is **18**: command `grep -rn '#\[ignore = "env-gate: tailwindcss' crates/ | grep -v ':[0-9]*: *//' | wc -l`. Five also require esbuild; reproduce that manifest count with `rg -n -F 'tailwindcss v4 + esbuild)' crates/CLAUDE.md | wc -l`. The `crates/CLAUDE.md` rows identify them, and the table calls out each by name. Three additional heavy tests are in `dev_dep_invalidation_1284_e2e.rs`; command `rg -c 'symptom C also needs Tailwind' crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs` confirms their ignore reasons name Tailwind only for symptom C. `#[ignore]` is the real default skip for tagged rows; normal test discovery omits them until their scheduled lane or an explicit ignored-test invocation runs.

### Tailwind-bound tests

| Test or suite | Kind | Disposition | Owner | Note: esbuild, real skip, scheduled home |
| --- | --- | --- | --- | --- |
| `crates/zfb/tests/css_command.rs` `css_command_output_is_deterministic_and_matches_committed_golden` | Tailwind env-gate | port | #3266 | No esbuild. `#[ignore]` skips default discovery; explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" step and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_highlight_class_default_and_inline_modes` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_explicit_sources_isolate_ambient_decoy` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_replays_consumer_entrypoint_with_explicit_sources` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_missing_input_exits_nonzero` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_zero_match_source_glob_exits_nonzero` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_same_canonical_input_output_exits_nonzero` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_missing_relative_import_exits_nonzero` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_real_unresolved_apply_exits_nonzero` | Tailwind env-gate | port | #3266 | No esbuild. Retarget to zudo-wind's leftover-directive diagnostic; delete only if #3246 makes the case unreachable. `#[ignore]` skips default discovery; explicit ignored runs require configured or staged Tailwind; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_atomic_success_replaces_and_failure_preserves_output` | Tailwind env-gate | port | #3266 | No esbuild. Same default skip (`#[ignore]`); explicit ignored runs require `ZFB_TAILWIND_BIN` or the staged Tailwind slot; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/css_command.rs` `css_command_matches_build_stylesheet_for_equivalent_explicit_source_plan` | Tailwind env-gate | port | #3266 | **Requires esbuild and Tailwind.** `#[ignore]` skips default discovery; explicit ignored runs require both binary slots; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" step and weekly T3 exam.yml exact-name filter. |
| `crates/zfb-css/tests/integration.rs` `subprocess_engine_against_real_binary` | Tailwind env-gate | delete | #3270 | No esbuild. `#[ignore]` skips default discovery; explicit ignored runs require the real Tailwind binary; health.yml T1 "Run ignored env gates (tailwind-only, workspace resolution)" step and exam.yml weekly T3 exact-name filter. |
| `crates/zfb-build/tests/prod_asset_graph_e2e.rs` `prod_asset_graph_with_real_tailwind_binary_against_fixture` | Tailwind env-gate | port | #3268 | No esbuild per the manifest tag. `#[ignore]` skips default discovery; explicit ignored runs require the real Tailwind binary; health.yml T1 "Run ignored env gates (tailwind-only, workspace resolution)" step and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/src/commands/build.rs` `default_runner_emit_prod_assets_returns_non_empty_css_for_real_project` | Tailwind env-gate | port | #3268 | No esbuild per the manifest tag. `#[ignore]` skips default discovery; explicit ignored runs require the real Tailwind binary; health.yml T1 "Run ignored env gates (esbuild + tailwind, workspace resolution)" step and exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/dev_sibling_watch_1678_e2e.rs` `e2e_dev_sibling_tailwind_utility_class_refreshes_served_css` | Tailwind env-gate | port | #3268 | **Requires esbuild and Tailwind.** `#[ignore]` skips default discovery; the test also returns early if `locate_esbuild()` or `locate_tailwind()` fails; exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/sibling_css_module_command_layer_build.rs` `sibling_only_utility_class_reaches_tailwind_source_scan_and_is_emitted` | Tailwind env-gate | port | #3268 | **Requires esbuild and Tailwind.** `#[ignore]` skips default discovery; the test returns early when `locate_esbuild()` fails, but it has no Tailwind self-skip, so an explicit ignored run needs `ZFB_TAILWIND_BIN` or the staged slot; exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/sibling_css_module_command_layer_build.rs` `sibling_generated_dir_utility_class_is_excluded_from_tailwind_source_scan` | Tailwind env-gate | port | #3268 | **Requires esbuild and Tailwind.** `#[ignore]` skips default discovery; the test returns early when `locate_esbuild()` fails, but it has no Tailwind self-skip, so an explicit ignored run needs `ZFB_TAILWIND_BIN` or the staged slot; exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/mirror_css_scan_mdx_e2e.rs` `sibling_mdx_utility_class_reaches_dev_css_scan` | Tailwind env-gate | port | #3268 | **Requires esbuild and Tailwind.** `#[ignore]` skips default discovery; the test also returns early if `locate_esbuild()` or `locate_tailwind()` fails; exam.yml weekly T3 exact-name filter. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs` `e2e_src_component_edit_rerenders_route` | heavy (ignore reason names Tailwind) | port | #3268 | Requires esbuild, not Tailwind: its actual early return is `locate_esbuild() == None`. The `heavy` `#[ignore]` tag suppresses default discovery; local T4 ignored-test command and exam.yml weekly T3 `quarantine-heavy` exact-name filter. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs` `e2e_transitive_css_import_refreshes_stylesheet` | heavy (ignore reason names Tailwind) | port | #3268 | Requires esbuild, not Tailwind: its actual early return is `locate_esbuild() == None`. The `heavy` `#[ignore]` tag suppresses default discovery; local T4 ignored-test command and exam.yml weekly T3 `quarantine-heavy` exact-name filter. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs` `e2e_new_utility_class_in_component_is_emitted` | heavy (ignore reason names Tailwind) | port | #3268 | Requires esbuild and Tailwind: it returns early if `locate_esbuild()` or `locate_tailwind()` fails. The `heavy` `#[ignore]` tag suppresses default discovery; local T4 ignored-test command and exam.yml weekly T3 `quarantine-heavy` exact-name filter. |
| `crates/zfb/tests/build_package_routes.rs` `package_route_page_tailwind_class_survives_in_stylesheet` | Tailwind self-skip | port | #3268 | Requires esbuild, Node and embedded V8; no Tailwind availability check. It returns early only when esbuild is absent, Node is unavailable, or `is_known_skip` identifies a known embedded-V8/esbuild limitation. Unignored in the normal T1 workspace suite; its binary is in the `e2e-heavy-unlocked` nextest group. |
| `crates/zfb/tests/dev_serve_injected_routes_e2e.rs` `dev_e2e_package_route_only_utility_class_reaches_dev_css` | Tailwind self-skip | port | #3268 | Requires esbuild, Tailwind and Node. It returns early if `locate_esbuild()`, `locate_tailwind()` or Node availability fails. Unignored in the normal T1 workspace suite; its binary belongs to the `e2e-heavy-locked` nextest group. |

The five env-gate tests tagged “requires esbuild” are the stylesheet-parity command case, sibling dev refresh, sibling source-scan inclusion, sibling generated-dir exclusion, and sibling MDX refresh. The four sibling cases are scheduled only in the weekly T3 lane; the first is in both T1 health and weekly T3. The two self-skip tests have no ignored-manifest row, so their real early-return behavior is recorded separately above.

### React-only, dead-path, and focused runtime suites

| Test or suite | Kind | Disposition | Owner | Note |
| --- | --- | --- | --- | --- |
| `crates/zfb-render/src/adapters/react.rs` React adapter unit tests, including React-only `pre_render_setup` guard | React-only adapter | delete | #3258 | Delete the obsolete React adapter and its assertions. |
| `crates/zfb/src/config.rs` React framework parse/default cases | React-only config | delete | #3258 | The old `framework: "react"` selection disappears with the config enum. |
| `packages/zfb/src/__tests__/content-render-markers-react.test.ts` | React-only SDK suite | delete | #3258 | No React marker runtime remains. |
| React arms in `crates/zfb-islands/src/bundler.rs` and `crates/zfb-islands/src/esbuild.rs` | React-only islands glue cases | delete | #3258 | Remove React branch assertions with the branch code. |
| Preact/React `Adapter::pre_render_setup` tests in `crates/zfb-render/src/adapters/preact.rs` and `adapters/mod.rs` | Dead adapter setup path | delete | #3259 | No production caller installs `globalThis.__zfbRenderToString`; this setup protocol is removed. |
| `crates/zfb-render/tests/render_smoke.rs` | Library renderer suite | delete | #3259 | Delete renderer-dependent cases; keep any isolated `SwcPipeline` case. |
| `crates/zfb-render/tests/mdx_loader.rs` | Library renderer suite | delete | #3259 | Delete loader/renderer-only coverage; keep any isolated `SwcPipeline` case. |
| `crates/zfb-render/tests/error_messages.rs` | Library renderer suite | delete | #3259 | Delete renderer-only coverage; keep any isolated `SwcPipeline` case. |
| `crates/zfb-islands/npm/test/hydrate.test.ts` | Alternate hydration package | delete | #3259 | No production importer for the private `@zfb/islands-runtime` package. |
| Tests inside `crates/zfb-islands/src/hydration.rs` | Dead HTML rewrite path | delete | #3259 | No production caller of the module's comment-sentinel/attribute-skeleton rewrite functions. |
| `bundle_per_island` tests in `crates/zfb-islands/src/esbuild.rs` | Dead bundling mode | delete | #3259 | Per-island bundles are not production-wired; port the live shared-bundle path instead. |
| `crates/zfb-render/src/swc_pipeline.rs` JSX runtime tests | MDX/library compile path | port | #3290 | Keep SWC and md-wasm compilation, retarget the runtime option/tests to zudo-react. |
| `crates/zfb-islands/src/bundler.rs` fallback test for `FrameworkKind::from_jsx_import_source` | Selection and glue | port | #3282 | Replace the silent Preact fallback assertion with an explicit owned-runtime contract. |
| Shared-glue text tests in `crates/zfb-islands/src/esbuild.rs` | Island glue | port | #3284 | Rewrite generated mount/unmount assertions for zudo-react. |
| `packages/zfb/src/__tests__/runtime.test.ts` throw-propagation cases | Lifecycle | port | #3281 | Rewrite assertions for per-island isolation. |
| Silent-drop cases in `packages/zfb/src/__tests__/island.test.ts` | Props transport | port | #3284 | Rewrite as named rejection cases for unsupported props. |
| `packages/zfb/src/__tests__/island.test.ts` remaining island transport cases | Island boundary | port | #3284 | Preserve the supported marker and props contract. |
| `crates/zfb-content/tests/mdx_jsx_emit.rs` | MDX emitter | port | #3283, #3288 | Retarget JSX dialect and generated runtime imports. |
| `crates/zfb-content/tests/mdx_jsx_emit_hast.rs` | MDX emitter/HAST | port | #3283, #3288 | Retarget generated JSX and trusted raw-HTML assertions. |
| `crates/zfb-islands/tests/preact_jsx_runtime_alias.rs` | Alias-only coverage | delete | #3288 | The temporary React-to-Preact runtime alias is removed. |
| `crates/zfb/tests/build_package_routes.rs` `*_share_*_preact_identity` tests | Runtime identity oracle | port | #3287 | Rewrite against zudo-react identity and embedded/staged package copies. |
| `crates/zfb/tests/build_package_routes.rs` `public_zudo_sg_catalog_renders_external_hook_story` | Published-consumer identity | delete | #3270 | Planner addendum assigns this test deletion to #3270; #3289 removes `packages/zfb-ssr-identity-fixture`. |
| `crates/zfb/tests/framework_packages_no_pnpm.rs` | Embedded packages | port | #3282, #3289 | Port zudo-react resolution assertions and keep the Hono half. |

### Remaining Preact-bound Rust test suites

Each path below is one suite named by the Preact/React census. Port its framework-specific fixture or expected output under #3287; #3286 owns the prerequisite inline-config/fixture sweep. Keep the suite's unrelated assertions.

| Test suite | Kind | Disposition | Owner | Note |
| --- | --- | --- | --- | --- |
| `crates/zfb-build/tests/bundler_anchor_check.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_class_mode_confirm.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_collection_seed_scope.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_consume_from_source_esbuild_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_css_modules.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_default_plugins.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_dot_path_staging_esbuild_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_dot_path_staging.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_exact_match_resolution.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_exclude_glob.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_injected_pages_root.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_integration.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_linked_package_alias_target_esbuild_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_main_fields.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_markdown_diagnostics.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_md_pages.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_node_modules_plugin_alias.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_node_modules_virtual_module.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_out_of_root_import_workspace.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_out_of_root_import.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_resolve_links.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_root_workspace_loose_file_alias_esbuild_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_root_workspace_stage_escape_audit_armed_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_root_workspace_stage_escape_audit_disarm_pin.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_shadow_session.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_shadow_tmpdir_isolation.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_sibling_mirror_esbuild_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_sibling_wholesale_mirror.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_staging_scan_memo.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_strip_md_ext.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_transform_compose.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_wasm_assets.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_workspace_pkg_alias.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/bundler_workspace_root_alias_esbuild_regression.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/dev_ssr_module_deps_workspace_staging.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/embedded_v8_snapshot_e2e.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/integration_e2e_routing_rendering.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/migration_fixes_integration_confirm.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/module_worker_workspace_first_party_roots.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb-build/tests/worker_only_routes_filter.rs` | zfb-build SSR/bundler integration | port | #3287 | Retarget Preact-specific bundle inputs to zudo-react; keep the suite's resolver and output assertions. integration_e2e_routing_rendering.rs regenerates and reviews its snapshot directory. |
| `crates/zfb/tests/build_cleans_outdir.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/build_package_routes.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/build_package_routes_consumer.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/build_terminates.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/check_command.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/client_bundling_cross_pipeline.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/client_router_autoinclude_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/collection_seed_3133_baseline.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/collections_outside_root_build_check_snapshot.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/css_modules_components_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/dev_build_static_parity.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/dev_poll_backend_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/dev_serve_injected_routes_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/dev_sibling_watch_1678_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/end_to_end_basic_blog_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/gfm_footnotes_tasklists_confirm_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/html_minify_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/mirror_css_scan_mdx_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/page_extension_full_matrix_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/page_extension_route_table_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/plugin_diagnostics_confirm_2375_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/preview_cross_mode_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/render_artifact_confirm_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/render_artifact_determinism_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/sibling_css_module_command_layer_build.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/ssr_reload_self_heal_e2e.rs` | zfb CLI integration | port | #3287 | Retarget the Preact config/output case; #3286 owns its fixture config, and the Tailwind-specific functions already have separate rows above. |
| `crates/zfb/tests/workerd_parity_e2e.rs` | zfb CLI integration | port | #3287 | The workerd fixture links Preact and preact-render-to-string from the framework tree; move it to zudo-react and preserve the Cloudflare adapter parity assertions. |
| `crates/zfb-islands/tests/exact_match_resolution.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/export_filter.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/integration.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/islands_user_tsconfig_paths.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/node_modules_plugin_alias.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/node_modules_virtual_module.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/npm_dist_islands.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |
| `crates/zfb-islands/tests/server_router_excluded_from_client_bundle.rs` | zfb-islands integration | port | #3287 | Port the framework-bound fixture or expected output; preserve the suite's scanner and bundler behavior. |

### Other framework-bound unit, TypeScript, md-wasm, and browser suites

| Test or suite | Kind | Disposition | Owner | Note |
| --- | --- | --- | --- | --- |
| `crates/zfb/src/config.rs` remaining Preact config parser/default cases | Config surface | port | #3286 | Re-target tests to strict `wind` parsing and hard rejection of stale engine keys. |
| `crates/zfb/src/commands/build.rs` non-React framework config cases | CLI config threading | port | #3286 | Remove the framework conversion/parameter assertions; preserve the CSS/build behaviors. |
| `crates/zfb/src/commands/dev.rs` inline framework config cases | CLI config threading | port | #3286 | Remove the framework config field from dev fixtures. |
| `crates/zfb/src/render_pipeline.rs` embedded runtime and framework-input unit cases | SSR test inputs | port | #3287 | Assert the owned runtime's embedded module resolution and identity. |
| `crates/zfb-build/src/bundler.rs` framework branch and synthetic-entry unit cases | SSR bundler | port | #3287 | Rewrite framework-specific branch assertions for the single runtime; drop dead manifest fields and hydrate shim expectations. |
| `crates/zfb-render/src/adapters/mod.rs` adapter contract and dispatch tests | Adapter contract | port | #3282 | Replace adapter dispatch with owned runtime entry constants/exports. |
| Preact adapter metadata tests in `crates/zfb-render/src/adapters/preact.rs` | Adapter contract | port | #3282 | Retain only assertions that describe live runtime entry metadata. |
| `packages/zfb/src/__tests__/content-render-markers-preact.test.ts` | Marker serialization | port | #3287 | Preserve the supported render-region and island marker contract. |
| `packages/zfb/src/__tests__/render-region-marker-cases.ts` | Marker test fixture | port | #3287 | Update fixture descriptions to the owned element dialect. |
| `packages/zfb/src/__tests__/config.test.ts` | Config validation | port | #3286 | Replace framework config expectations with the new `wind` schema and named stale-key errors. |
| `packages/zfb/type-tests/island-preact-fixture.tsx` | Type fixture | port | #3287 | Check the owned JSX and island types. |
| `packages/zfb/type-tests/config-fixture.ts` | Type fixture | port | #3286 | Check the new config shape. |
| `packages/zfb/tsconfig.preact-fixture.json` | Type fixture configuration | port | #3286 | Rename/update the JSX source fixture with the runtime contract. |
| `packages/zfb-runtime/src/__tests__/client-router.test.ts` | Runtime identity oracle | port | #3287 | Replace Preact branding assertions with the owned runtime identity. |
| `packages/zfb-runtime/src/__tests__/router.test.ts` | Server router suite | port | #3287 | Keep route behavior; retarget render-to-string integration. |
| `packages/zfb-runtime/src/__tests__/client-router/persist-island-lifecycle.test.ts` | Persisted island lifecycle | port | #3281 | Reassert identity, disposal and changed-props policy for zudo-react. |
| `crates/zfb-md-wasm/tests/api.rs` | md-wasm compile API | port | #3290 | Remove `jsxRuntime` option cases and update compile goldens. |
| `crates/zfb-md-wasm/tests/capabilities.rs` | md-wasm compile capabilities | port | #3290 | Port only framework-option cases; preserve unrelated capability assertions. |
| `crates/zfb-md-wasm/tests/parse_to_ast.rs` | md-wasm parser suite | port | #3290 | Update only any option setup that passes `jsxRuntime`; retain parse-to-AST behavior coverage. |
| `crates/zfb-md-wasm/npm/test/api.test.ts` | md-wasm API suite | port | #3290 | Remove the public option and refresh the relevant compile goldens. |
| `crates/zfb-md-wasm/npm/test/parse-to-ast.test.ts` | md-wasm parser suite | port | #3290 | Preserve parse-to-AST behavior and update shared option types as needed. |
| `crates/zfb-md-wasm/npm/test/consumer-compatibility.ts` | Packed consumer type fixture | port | #3290 | Verify the packed API no longer advertises `jsxRuntime`. |
| `crates/zfb-md-wasm/npm/test/consumer-compatibility-slim.ts` | Packed consumer type fixture | port | #3290 | Verify the slim entrypoint's option type matches. |
| `crates/zfb-md-wasm/npm/test/fixtures/packed-consumer/consumer.ts` | Packed consumer fixture | port | #3290 | Remove the obsolete option from the consumer example. |
| `tests/built-site-smoke/hydration.chromium.spec.mjs` | Real browser hydration | port | #3285 | Keep the real-build DOM identity, click and mounted-marker coverage for zudo-react. |
| `tests/md-wasm-browser-smoke/md-wasm-browser.chromium.spec.mjs` | Real browser md-wasm island | port | #3285, #3290 | Port the highlighter island and update the md-wasm emitted runtime fixture. |
| `tests/md-wasm-browser-smoke/stage-fixture-node-modules.mjs` | Browser fixture staging | port | #3285, #3289 | Remove Preact package staging and stage the owned SDK/runtime package. |
| `tests/md-wasm-browser-smoke/assert-staged-tarball.mjs` | Packed browser fixture assertion | port | #3285, #3289 | Assert packed zudo-react subpaths and runtime identity. |
| `tests/smoke/node-free-ts/run.sh` inline framework config heredoc | Node-free smoke fixture | port | #3286 | Remove the `framework` key while keeping the node-free scaffold smoke. |
| `docs/src/components/playground/__tests__/option-row-layout.test.ts` | Frozen docs-host suite | keep | #3242 (DD9) | Docs tests continue using the published Preact-based 2.x host during this epic. |
| `docs/src/components/playground/__tests__/playground-security.test.ts` | Frozen docs-host suite | keep | #3242 (DD9) | Keep the published 2.x renderer and its security assertions. |
| `docs/src/components/playground/__tests__/admonition-samples.test.ts` | Frozen docs-host suite | keep | #3242 (DD9) | Keep the published 2.x renderer and its examples unchanged. |

`crates/zfb-build/tests/integration_e2e_routing_rendering.rs` is in the Rust-suite table above; its `crates/zfb-build/tests/snapshots/e2e_routing_rendering/` HTML snapshots are ported and reviewed with that suite under #3287. The md-wasm test/golden migration includes `crates/zfb-md-wasm/tests/fixtures/parity/manifest.json`, its `reading-time.json` and `mdx-components-expressions.json` expected outputs, `fixtures/gfm-parity/manifest.json`, and `expected/footnotes-jsx.json`; those files are fixture data, not separate suites. The docs host's `docs/package.json` and `docs/tsconfig.json` remain on Preact by DD9. Framework-free router fixtures and tests whose only mention of “preact” or “react” is an arbitrary sample package name are not runtime-bound suites and are kept outside this migration manifest.

### Suites with framework-looking fixtures that remain unchanged

These census rows contain package-name or config examples, not Preact/React runtime behavior. Keeping them prevents a repository-wide keyword sweep from deleting generic resolver tests.

| Test or suite | Kind | Disposition | Owner | Note |
| --- | --- | --- | --- | --- |
| `crates/zfb-islands/src/scanner.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve tests using package names as scanner input; they do not execute Preact. |
| `crates/zfb-build/src/metafile_deps.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve generic package-resolution fixtures. |
| `crates/zfb-build/src/policy.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve package-name samples that are not runtime bindings. |
| `crates/zfb-types/src/audit_eligibility.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve the generic audit-policy examples. |
| `crates/zfb-plugin-resolver/src/lib.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve plugin resolution tests. |
| `crates/zfb-build/src/plugin_bundler.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve plugin bundler fixture names. |
| `crates/zfb-build/src/plugin_runner.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve plugin runner fixture names. |
| `crates/zfb-build/src/head_inject.rs` test module | Arbitrary package-name fixture | keep | #3287 | Preserve generic head-injection tests. |
| `tests/router-chromium/fixture/sync-dialog.html` browser suite fixture | Framework-free HTML | keep | #3242 (DD9) | It does not load either JSX runtime. |
| `tests/webkit-back-history/fixture/index.html` browser suite fixture | Framework-free HTML | keep | #3242 (DD9) | It does not load either JSX runtime. |
