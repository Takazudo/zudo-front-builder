# Bounded native utility adoption decision (#3833)

Decision date: 2026-10-06. Epic: #3825. Status: **approved contracts for #3834 and #3835; no production implementation or new browser evidence in this decision**.

## Inputs, ownership and evidence

The decision source is `3cfe07aa60046e642a3128b25cd93ad0203b5be5`, language spec 1/revision 12, migration vocabulary 4, compatibility profile `wind-preset-free@1.2`. Read-only GitHub refresh found main at `7f382cea6dfef01f1d9343bb603703f7e1d18b2d`. The Wind source tree has no difference between that main and the decision source. #3710, #3736 and #3809 are closed; PR #3766 merged as `a7befbeffa5c6a621b2e051412c0d6c577db5658`. The adoption owners are #3834 (display/appearance) followed by #3835 (flex/SVG/accessibility and final inventory refresh). #3838 separately owns reference promotion and reference-loader selection. External docs issues #3662/#3682 and docs-host owners #3328/#3329 are excluded.

Reviewed inputs: the current Rust catalog, resolver, token validation, migration vocabulary, candidate parser, ten-field sort key and authored reservation path; `tests/wind-compatibility/{profile,inventory.v1,upstream-surface.v1,runtime-keysets.v1}.json`; pilot/corpus manifests, observations and validators; and the #3832 harness confirmation log. Run `37391761485` confirms the existing harness at the decision source, including its then-expected `contents-gap`; it does not verify these future additions.

Independent reference probes executed Node against the verified cached npm artifact `tailwindcss@4.3.2`, SHA-256 `3673da9004404d12d4672bb5d002945319c2b3dea8c13ea022fdd33a3e260e62`, SRI `sha512-WtctNNSH8A9jlMIqxzuYumOHU5uGZyRv0Q5svQl+oEPy5w84YpBxdb7MdqyiSPQge5jTJ6zFQLq0PFygdccSBA==`, using empty theme, no preflight, explicit candidates. The source archive SHA-256 is `1d73680e19488b19e97ea3c96722e363cc0c4fd118af546857d8703e8f0f9be3`; observed tag commit is `056a1550721d4bf79ff732d5ab9414fa83f7064f`. The tag-to-npm relationship remains unverified. Probe records are committed beside this decision under `research/wind-native-adoption-reference/`. They establish compiler output only, not browser equivalence or Wind support.

## Common binding contract

1. No default tokens, implicit palette/scale, new token categories, registrations, resets, layers, extractor rules or variant classes. All adopted forms work with empty tokens and reset `none`. All existing valid token configurations and resolutions retain their meaning. Do not add their suffixes to global token-name reservations. Configured `colors`/`spacing`/`sizes` names have no effect on these additions, including `first`, `last`, `content`, `none`, `min`, `max`, `fit` wherever currently valid.
2. Preserve `wind.authoredClasses` exact **complete candidate** suppression before parser, resolver and diagnostics, for source, safelist, manifest and role origins. Reserving `table` does not reserve `hover:table`; reserving `hover:table` suppresses that exact candidate. The approved spellings already lie in migration vocabulary 4, so their transition from ZW014 to generation is explicit here. Do not claim additional authored names merely because a new root is registered. New exact static entries match only the entire utility name; their nonempty suffixes must fall through to the previous catalog/migration/ordinary-name behavior. Examples `table-widget`, `contents-panel`, `appearance-button`, `fill-logo`, `stroke-icon`, `not-sr-only-label`, `order-summary` and `order-1.5` remain ordinary source classes and unknown-explicit ZW008 in safelists/manifests unless some pre-existing rule already claims them. Keep newly introduced underscore-head recognition equally bounded: `contents_panel`, `table_widget`, `appearance_none`, `order_summary` and `fill_logo` must retain their old ordinary-name behavior instead of becoming ZW001 merely because a root was added. Existing prefix ownership such as `inline-*` is unchanged. `basis-*` retains its existing bounded-migration family's broad nonempty-suffix ownership; no expansion is needed.
3. Preserve canonical variant parsing/ordering: configured min/max breakpoints, configured dark, pointer, unnamed group/peer relations, supported states and pseudo-elements. Added utilities use own-element selectors and the existing escaping/variant emitter, including no implicit pseudo-element content. No new special treatment for appearance or SVG pseudo-elements. Require accepted examples `hover:contents`, `sm:appearance-none`, `focus:order-1`, `sm:basis-1/2`, `hover:fill-current`, `focus:not-sr-only`; configure `sm:640` explicitly. Preserve unknown-variant ZW002, duplicate/order ZW003 and existing unsupported named relation/attribute/arbitrary-selector/important ZW004 (R07–R11). Negative and slash rules below are additional value constraints, not variant extensions.
4. Preserve the ten-field sort tuple and specificity. New catalog positions below determine conflicts; class attribute arrival order never does. Permute and duplicate inputs in regression tests. Utility placement before/after authored CSS retains the existing configuration contract. No changes to #3386.
5. Accepted forms emit exactly the stated declarations and no ZW014. Value failures on an adopted shape use ZW005 with the stated rejection ID; retained migration forms use ZW014. Preserve origin policy: proven source class positions diagnose (ZW014 warning, promoted by `wind.strict`), low-confidence strings remain audit information, safelist/manifest entries fail, role classes remain tolerant, and authored reservations suppress before classification. Test each origin and strictness instead of assuming every invalid source string is fatal. Diagnostic messages must include the original candidate and recommend the bounded supported spellings or authored CSS as appropriate.
6. A malformed parser shape retains its current earlier diagnostic. A recognized adopted base with a disallowed negative uses R12, or disallowed slash R14; a valid bracket form deliberately deferred below stays ZW014. Examples distinguish parser errors, adopted value errors and retained gaps. Remove migration entries only for implemented spellings; preserve residual table-layout, order arbitrary, basis non-adopted, and SVG numeric/arbitrary coverage.
7. Preserve physical `mx-auto` left/right semantics, resolved vertical writing-mode observations, configured unused-token accounting, exact CSS trees, once-per-nonempty-stylesheet Wind prelude, UTF-8 served-byte checks, extraction independence and all mutation controls. #3372 ring/animation, #3386 layer/relation redesign and #3678 remain no-auto-adoption. No plugin/directive/arbitrary-selector uptake, package publication, production deployment or watcher activation.

## #3834: display and appearance

### Approved declarations and ordering

Each row is an exact static, with ID `v1.<candidate>`, no suffix value, no token/arbitrary grammar, no negative and no slash. Existing display entries stay unchanged.

| Candidate | Sole declaration |
| --- | --- |
| `contents` | `display: contents` |
| `flow-root` | `display: flow-root` |
| `list-item` | `display: list-item` |
| `table` | `display: table` |
| `inline-table` | `display: inline-table` |
| `table-caption` | `display: table-caption` |
| `table-cell` | `display: table-cell` |
| `table-column` | `display: table-column` |
| `table-column-group` | `display: table-column-group` |
| `table-footer-group` | `display: table-footer-group` |
| `table-header-group` | `display: table-header-group` |
| `table-row` | `display: table-row` |
| `table-row-group` | `display: table-row-group` |
| `appearance-none` | `appearance: none` |
| `appearance-auto` | `appearance: auto` |

Display entries use conflict group `display`, rank 1, order rank 0, joining existing display entries in root byte order. Appearance entries use group `interaction`, rank 40, order rank 0, root byte order. Thus at equal variant/specificity `table-row-group` follows `table-row`, `contents` follows `block`, and `appearance-none` follows `appearance-auto`; no arbitrary source-order winner. Add no vendor prefix or reset companion.

Reject `-contents`, `-table-cell`, `-appearance-none` with ZW005/R12; `contents/50`, `table-row/2`, `appearance-auto/50` with ZW005/R14. `appearance-[none]` and `display-[contents]` remain ordinary unknown families at strict explicit origins (ZW008), not new arbitrary support. `table-auto`/`table-fixed` remain ZW014 for the deliberately deferred table-layout property. Do not turn `table-widget` into a value error through the new `table` root. Existing diagnostic ownership of malformed `inline-*` forms stays intact.

### Required implementation and tests

Own `catalog/families/layout.rs`, appearance entries in `interaction.rs`, minimal exact-static resolver claim filtering, affected migration entries, focused catalog/compile/extract/audit tests and generated catalog. Test every positive table row with empty tokens, all listed rejections, exact authored reservations including variant spelling, ordinary suffix examples, source/strict/safelist/manifest/role diagnostics, conflicts and input permutations. Include extraction from HTML, TSX and MDX using the existing detection paths. Source detection must not broaden.

Add upstream corpus IDs `native-display` and `native-appearance`, both Chromium obligations. `native-display` enumerates the 13 display candidates above on separate target elements and checks one exact computed `display` per candidate (13 probes). Use valid table display context so computed values are not changed by flex/grid blockification. `native-appearance` has separate control elements with `appearance-none` and `appearance-auto`, checking one computed `appearance` each (2 probes). Exact generated trees must match the declarations above with the existing Wind prelude difference. Preserve the upstream display/appearance test-name and pinned-source provenance, and test independent negative compilation. Every static value is included; no single representative stands for all table modes.

The old corpus counts change only `upstreamCases:17 -> 19`, `upstreamProbes:40 -> 55`; pilot remains 15, supplemental 4, native guarantees 18. Existing IDs remain. Against the decision source, Chromium obligations become 56; Firefox/WebKit remain 10 each (76 total). These totals describe this source only; apply the integration preservation rule below when the incoming matrix repair expands existing engine obligations. New language-level variant/conflict tests supplement these browser fixtures. Manager/CI supplies Rust and browser execution.

### Required contents-gap transition and revision locks

Keep exact case ID `contents-gap` and its fixture path. In profile revision 3, transition its previous `implementation-gap`/expected-unsupported state to implemented `equivalent-shared`, token-free, exact `display:contents`, and the ordinary prelude reviewed difference. Its standalone CSS is now nonempty: one Wind layer-order prelude plus one `.contents` rule; reference has one `.contents` rule and no Wind prelude. Diagnostics are empty on both sides; computed `display` is `contents` on both sides. Update `pilot/observations.json` from Wind `inline` to `contents`. Preserve the old gap, profile 1.2 and run 37391761485 as history; never relabel that old evidence or delete/rename the ID.

Update every corresponding structural/diagnostic/expected-output branch in `differential-core.mjs`, `differential-runner.mjs`, `corpus-pilot.mjs`, profile reviewed difference membership, `pilot/manifest.json`, corpus `pilotPolicies`, fixture manifests, native/empty-token declarations and tests. Remove `contents-gap` only from the empty-output/absent-prelude and expected-ZW014 lists. Assert all 15 pilot IDs remain present exactly once and the new disposition is required; a stale report expecting unsupported must fail.

Implementation increments `SPEC_REVISION` 12 -> 13 (keep version 1), migration vocabulary 4 -> 5, profile revision 2 -> 3 (keep version 1). Record a dated #3834 history entry linked to this decision. Update current spec addenda/README/catalog export through their source/generator; do not rewrite the original historical spec revision 2 as though it had this implementation. Catalog revision/export identity and all affected fixture profile labels must agree.

Narrowly update the corresponding current-source `SPEC_VERSION`/`SPEC_REVISION` pins in `differential-runner.mjs` and `corpus-pilot.mjs`, profile source spec fields, `corpus-core.mjs` exact profile revision, reviewed manifest/structure/probe digests and fixed counts. Derive each reviewed digest from the authored expected contract and review it; never accept actual output wholesale as assertions. Add stale spec/profile/manifest/digest rejection tests. Do not loosen validators to accept any positive revision or derive their entire expected policy from the candidate report. #3838 owns loader/reference selection seams; preserve those changes during integration.

Regenerate the intermediate inventory from its source with spec/profile/catalog digests for this batch, using a committed source SHA containing the actual additions. It may mark implementation present/source-inspected; browser evidence remains null until real reports. #3835 will refresh both batches again to one final support-source pin. #3836 regenerates public EN/JA docs from source after that final refresh.

## #3835: flex, SVG and accessibility

### Order: approved bounded integer and keyword forms

Use `v1.order`, group `flex-item`, rank 5, order rank 2, own element, sole property `order`.

- `order-first` -> `-9999`; `order-last` -> `9999`; `order-none` -> `0`.
- `order-N` and `-order-N`: N is ASCII digits only, unsigned decimal numeric value 0 through 2147483647 inclusive. Leading zeros are accepted and canonicalized in the emitted number only. Positive emits canonical N; negative emits canonical `-N`, except `-order-0` emits `0`. No spacing unit or token lookup. Examples `order-01` -> `1`, `-order-01` -> `-1`.
- Only numeric forms permit negative. `-order-first`, `-order-last`, `-order-none` -> ZW005/R12. All slash modifiers on adopted forms, including `order-1/2`, -> ZW005/R14. Numeric overflow `order-2147483648` -> ZW005/R15.
- Do not claim `order` bare, `order-summary`, `order-1.5`, `order-1e2` or `order-+1` as valid numeric utilities. Preserve prior ordinary/parser behavior (ordinary source/ZW008 explicit for decimal and exponent names; forbidden outer punctuation remains parser failure). Restrict new root matching to existing foreign-value shapes: digits, first/last/none, or bracket for retained migration diagnostics. `order-[2]`, `order-[var(--priority)]` stay ZW014.

Ordering within this entry is the existing raw-candidate UTF-8 tie-break. `order-none` follows `order-last`; `order-2` follows `order-10`; `order-1` follows `-order-1`. Test these outcomes and document that order magnitude does not choose the cascade winner. No reorder of older catalog entries is authorized.

### Basis: approved native keywords and fraction grammar

Use `v1.basis`, group `flex-item`, rank 5, order rank 1, sole property `flex-basis`. It follows existing `flex-*`, `grow` and `shrink` rank-0 entries, so `flex-1 basis-auto` has `flex-basis:auto` at equal variants in either source order. Order's rank-2 entry writes a different property. No token categories, scale or arbitrary values in this batch.

| Candidate | `flex-basis` value | Reference/profile disposition |
| --- | --- | --- |
| `basis-auto` | `auto` | shared empty-theme form |
| `basis-full` | `100%` | shared empty-theme form |
| `basis-px` | `1px` | shared empty-theme form |
| `basis-0` | `0` | Wind native guarantee; empty-theme reference emits nothing |
| `basis-min` | `min-content` | Wind native extension; reference emits nothing |
| `basis-max` | `max-content` | Wind native extension; reference emits nothing |
| `basis-fit` | `fit-content` | Wind native extension; reference emits nothing |
| `basis-content` | `content` | Wind native extension; reference emits nothing |
| `basis-N/D` | `calc(100% * N / D)` | shared finite sample, exact structural difference below |

Fraction parts are ASCII digits only with integer value 1 through 1000000 inclusive, matching the existing fraction helper. Leading zeros canonicalize in the value; improper fractions are valid. No decimals, signed suffixes or zero parts. `basis-01/02` -> `calc(100% * 1 / 2)`; `basis-3/2` -> `calc(100% * 3 / 2)`. When a slash follows a digits/dots numeric-looking suffix, apply the fraction validator before keyword resolution and migration fallback: `basis-0/2`, `basis-1/0`, `basis-0.5/2`, `basis-1000001/2` -> ZW005/R13. `basis-auto/2` and other adopted keyword slashes -> ZW005/R14. Negative adopted keywords/fractions -> ZW005/R12.

`basis-1`, `basis-1.5`, `basis-hsp-sm`, `basis-[20px]`, `basis-[var(--width)]` remain ZW014, even with configured spacingUnit, spacing or sizes. There is no implied nonzero numeric scale or token binding. Keep the existing broad foreign `basis` entry for non-adopted suffixes. Add a native-guarantee boundary note: the reference compiler emits zero-part and over-limit fractions that Wind rejects; these are intentional bounded grammar differences, never positive parity cases.

### SVG: four exact presentation forms

Adopt only `fill-current` -> `fill:currentColor`, `fill-none` -> `fill:none`, `stroke-current` -> `stroke:currentColor`, `stroke-none` -> `stroke:none`. IDs are `v1.<candidate>`, exact-only roots, group `svg`, new rank 48; fill entries have order rank 0, stroke entries rank 1. No stroke-width, token lookup, opacity, arbitrary paint servers or registrations. At equal variants `fill-none` follows `fill-current` and `stroke-none` follows `stroke-current`.

Negative forms -> ZW005/R12. Slash on these exact adopted values, including `fill-current/50`, -> ZW005/R14. The reference supports opacity on current color with an extra `@supports` fallback, which is deliberately not adopted; retain the distinction in inventory. `fill-[red]`, `stroke-[2px]`, `stroke-0`, `stroke-1` remain ZW014. `fill-brand`, `stroke-brand`, `fill-transparent`, `stroke-inherit` retain ordinary/explicit-unknown behavior when not already in the bounded vocabulary, even if a same-named color token is configured. Do not make all `fill-*`/`stroke-*` names utility candidates.

### Accessibility: explicit reversal of current Wind clipping

Adopt exact `v1.not-sr-only`, group `miscellaneous`, rank 44, order rank 4, own-element selector, after existing sr-only rank 3. Emit exactly these declarations in this order:

```css
position: static;
width: auto;
height: auto;
padding: 0;
margin: 0;
overflow: visible;
clip: auto;
clip-path: none;
white-space: normal;
```

No border-width declaration: the helper cannot restore a project's previous border value. No content, visibility, display, outline or focus reset. `-not-sr-only` -> ZW005/R12; `not-sr-only/2` -> ZW005/R14. Suffix names remain ordinary under common claim filtering.

Keep current `sr-only` unchanged. The additional `clip:auto` is necessary because existing Wind sr-only uses `clip:rect(0,0,0,0)`; Tailwind 4.3.2 sr-only uses `clip-path:inset(50%)` and its reversal omits `clip`. Record `sr-reversal-clip-model` as an exact reviewed difference for standalone and composed reversal, with side-specific complete trees and clipping observations. This approves one added declaration, not dropping arbitrary extra CSS from comparison. At equal variants not-sr-only follows sr-only; `sr-only focus:not-sr-only` must reveal keyboard focus in the active state and remain clipped in the inactive state. Test both source permutations. This is CSS reversal coverage, not an assistive-technology conformance claim.

### Retained gaps and product dispositions

| Gap | Disposition and rationale | Owner and alternative |
| --- | --- | --- |
| table-auto/fixed and other table-layout utilities | Defer; distinct property from this reviewed display family | #3812; authored `table-layout:auto/fixed` |
| order arbitrary/custom-property/token forms | Defer; needs an explicit integer/custom-property validity and token contract beyond bounded native ordering | #3812; authored `order` |
| basis nonzero scale, named token, arbitrary values | Defer; needs precedence between sizes/spacing and an independently tested arbitrary length/intrinsic grammar | #3812; authored `flex-basis`, optionally using existing `--zw-spacing-*`/`--zw-size-*` variables |
| SVG named/transparent/inherit colors, opacity, arbitrary colors/URLs and stroke-width | Defer; mixed stroke color/width ownership and opacity fallback trees require a separate reviewed contract | #3812; authored `fill`, `stroke`, `stroke-width` |
| Broad Tailwind plugins/directives/arbitrary selectors | Intentionally exclude from this native addition; different language/configuration boundary | Existing owners, authored CSS |
| #3372/#3386/#3678 changes | Defer to their existing owners; this task grants no implementation permission | Keep issues and no-auto policy intact |

All five candidate groups receive concrete additions; no group is handed to a human for a product choice. Deferred rows stay visible in inventory and #3836 public docs, with tested existing diagnostics where recognized and exact authored reservations/alternatives elsewhere. No-preset policy is not the reason for any deferral.

### Required tests, differential coverage and revision locks

Test all finite keywords, order 0/1/01/2147483647 and negatives including zero, overflow, disallowed negative keywords, fractions 1/2,01/02,3/2,1/1000000,1000000/1 and invalid boundaries, SVG rejection/token-name preservation, exact reversal declarations, origin/strictness matrices, authored reservations, variants and input permutations. Preserve existing grow/shrink/flex/sizing/color behavior. Unit assertions must include exact declarations, IDs and sort keys, and no generated rule for retained gaps. Do not infer parity for rejected Wind forms that the reference generates.

Add five upstream corpus IDs after #3834:

| ID | Candidates and exact probe obligations | Engines |
| --- | --- | --- |
| `native-order` | Separate targets for order-0, order-1, -order-1, order-first, order-last, order-none; computed order 0,1,-1,-9999,9999,0 (6 probes) | Chromium |
| `native-basis` | Separate flex items for basis-auto/full/px/1/2; computed flex-basis auto,100%,1px,50% (4 probes) | Chromium |
| `native-svg` | Four SVG targets for the adopted forms under authored color rgb(18,52,86); computed fill/stroke rgb(18,52,86) or none (4 probes) | Chromium |
| `native-not-sr-only` | Standalone not-sr-only; computed position static, clip auto, clip-path none, white-space normal (4 probes), exact full declarations independently | Chromium, Firefox, WebKit |
| `native-sr-reversal` | sr-only + focus:not-sr-only on a keyboard-focusable target; inactive/active position, clip, clip-path, white-space (8 probes), with inactive Wind clip rect(0px, 0px, 0px, 0px) / reference auto and Wind clip-path none / reference inset(50%); active both static/auto/none/normal; inactive both absolute and nowrap | Chromium, Firefox, WebKit |

The exact-tree reviewed differences additionally account for negative order's Wind literal `-1` versus reference `calc(1 * -1)`, fraction multiplication spelling/order, currentColor/currentcolor serialization and existing Wind prelude/variant nesting. They do not normalize away unrelated rules/declarations. Structure expectations come from the pinned reference compiler plus this approved Wind contract; browser probes remain independent. Add exact negative reference assertions for the selected rejected/shared forms, and retain explicit difference assertions where upstream accepts Wind-rejected forms.

Add five declaration-only native IDs `native-basis-zero`, `native-basis-min`, `native-basis-max`, `native-basis-fit`, `native-basis-content` to the existing empty-token/native guarantee lane, with exactly the five sole-property writes in the basis table. Add their empty-theme reference absence checks; do not call those native rows equivalent or browser-verified. Existing native guarantee semantics stay declaration-only.

After this batch, upstream cases 19 -> 24, probes 55 -> 81, native guarantees 18 -> 23. Pilot remains 15 and supplemental remains 4. Against the decision source plus these additions, Chromium obligations become 66, Firefox and WebKit 12 each (90 total). Apply the integration preservation rule below when the incoming matrix repair adds existing engine obligations. Preserve all older IDs and controls. All other counts are unchanged. Explicitly add the two reversal IDs to targeted engine policy. Missing or duplicate new IDs, old profile policy and mismatched counts fail.

Increment language spec revision 13 -> 14, migration vocabulary 5 -> 6, profile revision 3 -> 4. Append dated #3835 history; retain the #3834 contents-gap transition. Update catalog/spec addenda and generated reference sources, all profile labels, narrow current-source spec/revision pins and corpus fixed counts/digests exactly as required for #3834. Fresh input identity must invalidate revision-13/profile-1.3 and older reports. Preserve #3838 reference selection changes.

As the final catalog mutator, #3835 owns the complete support-source SHA refresh across **both** batches. First commit the implemented source/catalog/profile/fixtures, then point `scripts/wind-compatibility/inventory.mjs` WIND_SOURCE_SHA and inventory evidence rows to that full committed SHA, and regenerate/validate inventory in a follow-up commit. The pin is a committed source snapshot containing both additions, not the hash of its own inventory commit. Prove its catalog/source digests equal the final checkout's relevant implementation; no stale revision-12/pre-adoption support rows. Refresh all rows' source identity coherently, but change implementation dispositions only where code exists. The partial order/basis/fill/stroke pattern rows must state the supported subset and retained exclusions instead of suggesting whole-family coverage. Assign retained gap rows tracking issue #3812 and authored alternatives; keep source-inspected, contract-tested and differential-tested fields separate. #3836 must reject stale pin/digests before generating bilingual public data.

## Execution and verification handoff

**Incoming matrix repair:** the manager identified missing existing mandatory Firefox/WebKit pilot/control coverage after the decision-source harness confirmation. Its independent repair must be preserved. The exact additions here are #3834: two Chromium upstream cases / 15 upstream probes; #3835: five Chromium upstream cases / 26 upstream probes, two new targeted cases in each Firefox/WebKit leg, and five declaration-only native guarantees. Recompute reviewed total obligations from the repaired predecessor plus these exact additions; never reset the repaired matrix to the historical 10/12 targeted-case counts above. Preserve all repaired pilot/control obligations and their report validation. If that intervening reviewed work consumes a planned profile revision, use its immediate next revision for each adoption batch, record the predecessor and new values in history, and update every validator/pin consistently. Language spec revisions 13/14 and migration versions 5/6 remain reserved for these batches unless another integrated language change already consumed one; then use the same immediate-successor rule with an explicit source/history record. This is a deterministic sequencing instruction, not permission to broaden semantics or accept arbitrary revisions.

The downstream issue bodies contain the common contract and their complete owned section from this document. They are implementation tasks after prerequisite integration. Read root CLAUDE.md and scoped instructions; revalidate main/ownership read-only. Only the manager pushes/merges. Children commit locally and run inexpensive affected Node/source/format checks. Cold Rust compilation, browser/e2e, full builds and heavy suites belong to the manager's shared guarded lane/CI. Required fresh-context review covers gate-defining changes, particularly profile policy/digests/counts and contents-gap. Never obtain a pass by widening tolerance, dropping IDs, accepting stale evidence or copying actual output into expected data. No Git LFS or large retained build artifacts. Report unrun Rust/browser checks explicitly; this decision itself supplies no execution evidence for Wind additions and admits no baseline.
