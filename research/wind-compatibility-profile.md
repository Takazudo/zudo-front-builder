# Wind preset-free compatibility profile

Ratified by [#3826](https://github.com/Takazudo/zudo-front-builder/issues/3826),
2026-10-06. Canonical identity: [`tests/wind-compatibility/profile.json`](../tests/wind-compatibility/profile.json),
`wind-preset-free`, **version 1, revision 1**. This is a policy commitment with
pending evidence, not an admitted compatibility baseline.

## Product boundary and authority

Wind provides familiar native CSS utilities, project-owned semantic tokens, and
selected Tailwind-compatible behavior. No implicit palette, spacing scale, font
scale, or breakpoint is introduced. Common static utilities remain useful with
empty tokens: `block`, `hidden`, `inline-flex`, supported auto margins, constants
such as `p-0`, and count-based `grid-cols-2`. `hidden` means `display:none`; `none`
is not its utility spelling. Existing supported token-free utilities remain
owned language guarantees even where they are outside this profile's measured core.

Prefer project names such as `p-hsp-sm`, `gap-vsp-md`, and `bg-surface`. Explicit
`spacingUnit` remains an optional numeric scale; it is neither required for static
utilities nor silently inserted into semantic examples. The owner's
[Tight Token Strategy](https://zudo-css-wisdom.takazudomodular.com/docs/design-tokens/tight-token-strategy/)
informs this vocabulary. Its build-error examples are not compiler evidence:
unknown source candidates must be independently probed. Tailwind's
[theme documentation](https://tailwindcss.com/docs/theme) distinguishes theme-backed
utilities from static utilities, but an exact pinned compiler is the reference oracle.

The [v1 language spec](3242-zudo-wind-v1-spec.md) and
[owned-engines ledger](3242-owned-engines-ledger.md) retain their dated decisions.
Their initial closed catalog is an implementation boundary, not a permanent
philosophical rejection of omitted native CSS. This profile records compatibility
promises separately. Adopting a utility still requires a bounded language decision,
collision/diagnostic review, implementation, and evidence; upstream availability
alone does not authorize it.

Execution baseline is `a2642d02270aa4fc74fa5de049d4482da087af1f`, after the manager's
#3710/#3736, #3843 and #3847 integrations. Current `crates/zudo-wind/src/lib.rs`
declares language spec **1**, revision **12**. The historical document's revision 2
is not the current implementation revision. Inventory must separately pin the
actual Wind source SHA, exported catalog digest, and language identity.

## Versions, dispositions, and evidence

`schemaVersion` describes the JSON shape. `profileVersion` changes when existing
promises are removed, narrowed, or given broader semantic exceptions.
`profileRevision` changes for additive core membership, new mappings, assertion or
browser-pin changes, and other reviewed policy refinements. A new version starts
at revision 1. Record each policy change in JSON history and explain affected cases;
editorial corrections with no policy effect need no bump. Neither number follows
Tailwind releases, Wind spec revisions, nor catalog generation.

An implementation fix restoring the current promise changes source/evidence
identity, not the promise. A utility addition outside the measured core changes
language/catalog support without automatically expanding this profile. Evidence
refreshes and checked reference promotion do not themselves change profile version;
changed inputs invalidate earlier reports. Changing a promised semantic outcome,
accepted difference, mapping, tolerance, or required coverage needs explicit policy
review and a version/history decision before accepting the new result. Gate-defining
changes additionally require fresh-context review under root `CLAUDE.md`.

Every inventory row separates these axes:

| Axis | Required meaning |
| --- | --- |
| Disposition | `equivalent-shared`, `equivalent-mapped`, `intentional-difference`, `implementation-gap`, or `out-of-scope`; a comparison contract, not a test result. |
| Implementation | Implemented or not implemented at an exact Wind SHA; absence of a project token does not mean the family is unimplemented. |
| Configuration | Exact Wind config, reference theme mapping, reset choice, and required/absent tokens, scale, or breakpoints. |
| Evidence | `source-inspected`, `contract-tested`, and `independently-differential-tested` are separate facts with report identities. None implies the next. |

`equivalent-shared` means intended equivalent behavior in the declared environment;
structural differences may still be counted explicitly. `equivalent-mapped` needs
an explicit, minimal configuration mapping and browser evidence. An
`intentional-difference` must assert both outcomes. An `implementation-gap` remains
visible with its adoption/defer owner. `out-of-scope` syntax (for example Tailwind
plugins/directives) is inventoried with a reason; it is not a passing comparison.

Consumers must reject unknown schema/profile versions, unknown dispositions, duplicate
case IDs, missing configurations and inconsistent difference back-references. The
JSON owns machine identity and membership; this document owns their meaning. A
conflict between them blocks assessment until explicitly reconciled.

The initial JSON records Wind source inspection only. Its null contract/differential
fields mean **unverified**, even where older suites exist. Reference planning probes
in #3825/#3826 are provenance hints, not independently reproduced reports for this
profile. Each new report carries the source, profile, catalog, reference, adapter,
fixture, source-input, configuration, assertion, lock, reset and browser identities
listed in `evidenceIdentityRequired`. Nonbrowser checks mark browser inputs explicitly
not applicable; browser checks cannot do so. Wind-only contract checks explicitly mark
reference inputs not applicable and cannot count as differential evidence. Unknown
upstream Git provenance remains
explicit and requires a recorded provenance decision; never invent a Git SHA.

## Worked contracts and guarantee membership

The JSON `requiredCases` IDs below are stable minimum core obligations. A case's
configuration is part of its identity. `empty` means no tokens, spacing unit, or
breakpoints, Wind reset `none`, reference default theme omitted and preflight off.
All assertions remain pending independent execution.

| Case ID / spelling | Disposition | Required outcome and configuration |
| --- | --- | --- |
| `block` / `block` | equivalent-shared | Empty config: `display:block`. |
| `hidden` / `hidden` | equivalent-shared | Empty config: `display:none`, including visibility/geometry. |
| `inline-flex` / `inline-flex` | equivalent-shared | Empty config: inline flex container, with child layout. |
| `mx-auto` / `mx-auto` | intentional-difference | Empty config: Wind physical left/right; Tailwind logical inline margins. Test horizontal and vertical writing modes, not only coincident horizontal output. |
| `grid-cols-2` / `grid-cols-2` | equivalent-shared | Empty config: two `minmax(0,1fr)` tracks; fixture supplies `display:grid`. |
| `p-0-empty` / `p-0` | intentional-difference | Wind guarantees zero padding without tokens; empty-theme Tailwind 4.3.2 planning probe omits it. Assert reference absence and Wind's effect over nonzero padding. |
| `p-0-mapped` / `p-0` | equivalent-mapped | Wind stays empty. Reference declares only `--spacing-0:0px`. Independently verify generation and computed zero; never inject a corrective `.p-0` rule. |
| `named-spacing` / `p-hsp-sm` | equivalent-mapped | Wind `tokens.spacing.hsp-sm = 17px`; reference `--spacing-hsp-sm:17px`. No numeric scale. |
| `named-color` / `bg-surface` | equivalent-mapped | Wind `tokens.colors.surface = #123456`; reference `--color-surface:#123456`. No stock palette. |
| `unconfigured-p-4` / `p-4` | intentional-difference | Supported numeric spacing family lacks its explicit unit. No utility rule on either side; probe and assert diagnostics separately by origin/strictness. |
| `undeclared-palette` / `bg-gray-500` | intentional-difference | Supported color family lacks a declared token. No utility rule on either side; no implicit gray palette or presumption of a Tailwind error. |
| `configured-p-4` / `p-4` | equivalent-mapped | Opt-in only: Wind `spacingUnit:0.25rem`, reference `--spacing:0.25rem`; padding computes to `1rem`. |
| `contents-gap` / `contents` | implementation-gap | Token-free reference utility absent from this Wind catalog. Adoption decision #3833 owns it; no missing-token explanation or silent exclusion. |
| `hover-block` / `hover:block` | equivalent-shared | Empty tokens; hover state and `(hover:hover)` capability both control activation. |
| `breakpoint-block` / `sm:block` | equivalent-mapped | Wind breakpoint `sm:640`, reference `--breakpoint-sm:640px`; below, at, above threshold. |

Static keywords, intrinsic constants (`0`, `px`, `auto` where supported), finite
counts/ratios, project tokens, and optional numeric scaling are distinct inventory
categories. Arbitrary values are bounded owned grammar, not an implicit theme.
Variants have their own activation/specificity contracts and must not inherit a
base utility's evidence automatically. Composed systems (ring/animation), engine
directives and plugins are separate decisions, not unexplored token namespaces.

## Structural and behavioral comparison rules

Keep exact reviewed-difference IDs and case membership in JSON. Count each observed
difference occurrence and distinct ID; report expected and actual occurrence totals.
A reviewed difference is successful verification of two stated outcomes, not equality.
An extra selector, missing declaration, changed condition, or expanded exception fails.
Do not normalize away custom properties, layers, resets, specificity or source order.

Current source corrects a planning-era claim: **Wind named tokens use variables**
(`catalog/resolve.rs` → `var(--zw-...)`, `token_vars.rs` → configured variables in
`zw-tokens`), whereas intrinsic zero and numeric spacing use literals. Therefore:

- Mapped `p-0`: reference `var(--spacing-0)` and its theme rule versus Wind literal
  zero; reference padding shorthand versus Wind four physical longhands.
- Named spacing/color: both sides use variables, with different namespaces and
  theme-rule/layer structure. The reference `:root, :host` rule holding only explicit
  tokens is expected; it is not a leaked stock theme. Compare computed values and
  nested token-scope controls; do not claim interchangeable custom-property APIs.
- Numeric scaling: reference variable calculation versus Wind computed literal.
  Runtime overrides of these implementation variables are not promised equivalent.
- Variants: reference nested selectors/media versus Wind flat media/rules. Structural
  analysis must resolve nesting while preserving conditions, specificity and order;
  activation still needs browser evidence. CSS text equality is insufficient.
- `mx-auto`: preserve and count physical versus logical writes. Use `horizontal-tb`
  and `vertical-rl`, each in `ltr` and `rtl`, with constrained boxes, computed physical
  margins and measured geometry. Horizontal coincidence cannot certify vertical parity.

Composition cases must exercise overlapping utilities, reordered class strings,
authored CSS before/after utilities, selector specificity, nested children and
state transitions. Do not compensate one compiler's result with generated corrective
CSS or feed Wind catalog declarations to the reference compiler. Shared HTML and
explicit configs are permitted; expected behavior must have independent provenance.
Source scanning and explicit candidates are separate adapters, with origin-preserving
diagnostics. Distinguish an ignored source literal, proven class, safelist, manifest,
and invalid config; strictness and authored-class ownership matter. Fixes must
preserve the inherited source-isolation/diagnostic contracts.

## Bounded coverage and browser contract

The minimum core is every JSON `requiredCases` row plus `requiredControls`.
#3828/#3831 enumerate concrete inventory/fixture IDs before assessment. Expanding
core beyond this minimum requires profile revision/history review; an inventory's
existence does not silently make every supported grammar string a differential claim.
All catalog support remains visible, including unmeasured rows and omissions.

For selected finite keyword/count domains, enumerate all selected members. For open
numeric, token, arbitrary-value and variant domains, commit deterministic samples:
zero/unit/nonzero/decimal and valid/invalid boundaries where applicable; missing and
present semantic tokens; variant inactive/active states; breakpoint below/at/above;
and interactions crossing axes, scope and cascade. State each sampled domain and its
limits. No random or model-selected subset at assessment time, implicit Cartesian
product, or inference that samples prove every class. `grid-cols-2` promises that
selected count; adopting the full 1–12 domain requires all twelve cases.

Required execution matrix (policy, **not execution evidence**):

| Host | Engine | Coverage | Observed Playwright browser revision |
| --- | --- | --- | --- |
| Linux, Ubuntu 24.04 x64 | Chromium | Entire declared core, including gaps, intentional differences and negative controls | 1228; browser 149.0.7827.55; headless shell also 1228 |
| Linux, Ubuntu 24.04 x64 | Firefox | Targeted composition, controls and writing modes | 1532; browser 151.0 |
| Linux, Ubuntu 24.04 x64 | WebKit | Same targeted obligations as Firefox | 2311; browser 26.5 |

Targeted means at least mapped zero and semantic tokens in nested scopes, hover and
breakpoint activation, grid/inline-flex composition, authored-cascade/specificity
controls, the complete `mx-auto` writing-mode set, and native reset/form controls.
Each engine must demonstrate a deliberately wrong value or missing rule is detected.
All engines need explicit fixture IDs; these labels cannot become discretionary skips.

`pnpm-lock.yaml` resolves `@playwright/test`, `playwright` and `playwright-core` to
**1.61.0**, despite the looser package.json range. JSON records lock digest, core SRI,
and the inspected existing installation's `playwright-core/browsers.json` digest.
These revisions were read, not downloaded or executed in #3826. Runtime reports
must verify the installed manifest and resolved host override, executable identity,
actual version, headless mode, viewport and media inputs. WebKit overrides for other
hosts must not be mistaken for Linux 2311. A host/pin change requires review and fresh
evidence. macOS, Windows, mobile, other distributions/architectures, and engines
outside this matrix remain visibly unverified; Linux WebKit is not Safari certification.

## Reference lifecycle, resets, and fail-closed admission

Choose test-only **`tailwindcss@4.3.2`** as the deliberate initial candidate. The
transitive browser package in the lock explains the choice; it does not establish a
historical consumer baseline. #3827 verifies the tarball SRI hint recorded in JSON
and exact source identity. Registry Git identity was absent during planning; null
is honest until independently resolved. Use deterministic version resolution,
stable releases by default, prereleases only when requested.

The reference adapter omits the default theme, compiling utilities with only the
explicit mapped theme. Planning's `@theme { --*: initial; } @tailwind utilities;`
probe is supporting history; validate the actual omission adapter independently.
Shared utility tests use Wind reset `none` and reference preflight off. Native
migration tests separately retain and record Wind reset `minimal-v1`/`owned-v1`
and reference preflight on/off as explicit fixtures, including headings, lists,
borders and form controls. Never suppress reset differences by globally disabling
these migration tests or importing a stock theme.

`referencePolicy.initialState` is the immutable bootstrap state: accepted reference
and reviewed-through are both **unset** (`null`). #3827/#3838 own separate live state
records. Candidate assessment alone advances neither. Checked admission requires
complete mandatory evidence and exact profile/report identities. Reviewed-through
records completed assessment/disposition, including rejected candidates; accepted
reference records only a checked admitted baseline. Their transitions are distinct.

Missing manifests/cases, empty unexpected CSS, skipped engines, stale identities,
missing assertions and unexplained differences fail the compatibility assessment.
Expected omission cases must execute and assert omission. Aggregate accounting must
include every planned case exactly once with per-engine results, distinguishing
matches, mapped matches, reviewed differences, gaps, out-of-scope and failures.
A failed supported case cannot be relabeled an exclusion or given a looser tolerance
to obtain green. Deferred mandatory evidence blocks admission even if development
integration proceeds. Artifacts remain local/ignored or normal CI artifacts; no LFS.

Preserve #3372 composed ring/animation and #3386 cascade redesign as deferred,
with no automatic adoption. #3833 reconciles existing owners before any adoption.
Optional #3822 watcher remains inactive. Existing #3696 docs ownership and
#3328/#3329 docs-host migration stay outside this work; #3836 owns this epic's new
bilingual guidance. This policy installs no compiler/runtime dependency, implements
no utility or harness, activates no watcher, and changes no release/ruleset policy.

## History

| Profile | Date | Decision |
| --- | --- | --- |
| 1 / 1 | 2026-10-06 | #3826: initial bounded preset-free promise, exact required cases/differences and browser scope; source correction for Wind named variables; no compatibility baseline admitted. |
