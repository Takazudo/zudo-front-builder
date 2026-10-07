# Factory target contract: local static spread and an explicit composition migration

Decision for [#4005](https://github.com/Takazudo/zudo-front-builder/issues/4005),
under [#4004](https://github.com/Takazudo/zudo-front-builder/issues/4004).
Evidence baseline: zfb `66270bc37b8cc82545af0165190c4472ada37ea9`.
Consumer baseline: `zudolab/zudo-doc@8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761`.
This is a specification, not a claim that the implementation or downstream
migration has already passed. No consumer source tree is copied into zfb.

## Decision

Choose **A + D**. Add one level of spread from a proven local plain-object `const`
to the existing literal factory argument. Require a downstream migration for
conditional host components, forwarded dependency parameters, and opaque maps:
select rendered fixed-target boundaries at the composition site; pass their
children through server-only slots and settings factories.

This is an explicit **required downstream migration**, including a public seam
change for consumers whose package currently accepts an arbitrary host component
and owns its Island mount. It is not a claim of drop-in compatibility for the
zudo-doc application. Its own browser, route, plugin, and release checks remain
necessary after that migration.

The dependency graph stays decision → {scanner #4006, docs #4007} → confirm #4008.
All three remain necessary; no SKIP marker or B/C graph re-decomposition applies.
The manager applies the reviewed replacement issue bodies before starting Wave 2.

Hard non-goals, verbatim:

- functions never cross JSON island props;
- no arbitrary factory execution;
- no registering every client export.

Preserve the plain-literal rule (#3895/#3919), the direct Island control, the
opaque-container rule, and identity validation. A failed proof remains a failing
production build, never a valid partial registry.

## Pinned consumer evidence and path correction

These files were read through GitHub's contents API at the full consumer SHA:

| Source | Observed shape and consequence |
| --- | --- |
| [chrome/derive.tsx:470–501](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/packages/zudo-doc/src/chrome/derive.tsx#L470) | Local dependency objects are spread into `createBodyEndIslands`, `createDesignTokenPanelIsland`, and `createThemePackSwitcherIsland`. A can admit the object syntax, but not the dynamic selected component. |
| [chrome/derive.tsx:443–476](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/packages/zudo-doc/src/chrome/derive.tsx#L443) | Host component wins over the fallback; a package-default suppression predicate can choose `undefined`. An explicit host override must still win when only the default is suppressed. |
| [design-token-panel-island.tsx:39–62](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/packages/zudo-doc/src/doc-body-end-islands/design-token-panel-island.tsx#L39) | Reads `deps.DesignTokenPanelBootstrap`, checks the settings/component gates, then owns `Island({ when: "load", children: <DesignTokenPanelBootstrap /> })`. Registering the fallback alone cannot prove the host path. |
| [src/chrome-bindings.tsx:107–125](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/src/chrome-bindings.tsx#L107) | Root `mdxExtras` begins at line 115; it stores `DetailsWrapper`, `IslandWrapper`, and `PresetGeneratorFallback` among ordinary components. |
| [hostpanel chrome-bindings.fixture.tsx:117–135](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/e2e/fixtures/hostpanel/src/chrome-bindings.fixture.tsx#L117) | The tracked host fixture's map begins at line 125 and contains the same wrapper entries. |
| [pages/lib/_preset-generator.tsx](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/pages/lib/_preset-generator.tsx#L60) | `PresetGeneratorFallback` directly returns an `Island(...)` call around a fixed client target; it is a concrete map boundary value needing the Fragment recipe. |
| [v4.1-integration.md](https://github.com/zudolab/zudo-doc/blob/8e9b5bc0c7f3c00fd9311f79b66a4a998f3ae761/docs/findings/4430-zfb3-migration/v4.1-integration.md) | Direct and literal-factory controls build; spread and conditional controls fail; static default/host boundaries through a child slot build. Six E2E fixtures fail before browser startup at opaque maps. Those observations establish no browser parity. |

The issue's `packages/zudo-doc/src/chrome-bindings.tsx` does **not** exist at this
SHA (contents API returned 404). The recursive pinned tree has
`packages/zudo-doc/src/chrome-bindings.ts` (the API), `src/chrome-bindings.tsx`
(the root bindings), and `e2e/fixtures/hostpanel/src/chrome-bindings.fixture.tsx`
(the tracked host fixture). The findings' generated fixture paths are not
misrepresented as checked-in files. The two map sources above supply the actual
line-115/125 evidence. The root `IslandWrapper` merely returns `props.children ?? null`; current wrapper summarization treats that as Ordinary. Do not attribute the error to its `Island` key. The additionally inspected `_preset-generator.tsx` provides a direct-return boundary value in that map; no attempt is made to reproduce the complete downstream build in this decision.

Minimal consumer target-selection shape (type casts omitted):

```tsx
const designTokenPanelDeps = {
  DesignTokenPanelBootstrap:
    ctx.hostBindings.DesignTokenPanelBootstrap ??
    (skipsPackageDefaultDesignTokenPanel(ctx.settings) ? undefined : DesignTokenPanelBootstrap),
};
```

A local immutable object does not make this selected value a single statically
known function. The chosen contract must preserve both absence and host selection
through a migration, rather than silently substituting the package default.

## Options evaluated against the scanner

`R` below means [registration.rs](../crates/zfb-islands/src/scanner/registration.rs)
at the zfb evidence baseline. Line references intentionally describe that base.

| Option | Consumer shapes unblocked | Complexity and identity proof | Diagnostic and budget impact | Decision |
| --- | --- | --- | --- | --- |
| A: one-level local const spread | Plain local data/target dependency objects whose final target resolves to the same definition at every call. Does not unblock the real conditional member or forwarding chain by itself. **Opaque-container fixtures are NOT unblocked.** | Bounded indexed object facts and reference/write validation; still one `Value::Function`. R:306–333, 1468–1484, 1574–1610. | Category 8 admits a closed additional syntax; existing reason order remains. Cache source facts; extend 200-call and linear AST budgets. | Accept. |
| B: A plus nested member proof | Could potentially carry `param.member ?? LocalDefault` through forwarded parameters, but only after every caller/override/absence branch is proven. **Opaque-container fixtures are NOT unblocked.** | Existing `member_from_expr` handles one immediate parameter and one static property (R:235–267); parameter forwarding currently counts as escape (R:1327–1402). Requires a new interprocedural provenance and optional/candidate contract. | New precedence between unresolved callers, absence, cycles and conditional branches; new cross-module/chain budgets. A's caches alone are insufficient. | Reject here: the required every-mounted-target proof is not established. |
| C: union for `??`, ternary, `\|\|` | Finite identifier alternatives only; does not prove `ctx.hostBindings.X`, so does not independently unblock the consumer. **Opaque-container fixtures are NOT unblocked.** | `Value::Candidates` would affect cache/renaming (R:1129–1142), agreement (1668–1677), `Child`/merge (579, 2003), `resolve_form` (2161), registry insertion (3349–3358). Every candidate still needs a client route. | Changes the conditional diagnostic and the single-target JSX policy in acceptance:1003–1044; needs candidate-width/route/cache budgets. | Reject: larger independent contract change without solving the reported consumer. |
| D: composition migration, no additional scanner relaxation | Caller-owned static default/host Island boundaries, package-owned server settings gates, child slots, and replacement of bare-return or generic forwarding map boundaries with fixed-target Fragment wrappers. | Existing direct boundary discovery and runtime identity checks prove each mounted target. No evaluation or function-prop transport is introduced. | Rejected shapes retain diagnostics and gain tested rewrites. No resolver budget increase. | Accept alongside A; downstream public seam migration is explicit. |

**B proof criterion applied:** EVERY possible mounted target is proven registered,
including an explicit host override. A trace ending at the fallback while the
host value remains `Other`, opaque, unvisited, or runtime-dependent fails this
criterion. The pinned code permits an optional host function through `ctx` and
conditionally omits the fallback. Neither reading its default import nor forming
a union of the visible fallback expressions proves that host function. B is
therefore not selected. Choosing it later requires a new decision and graph:
provenance/absence proof → candidate and registry representation → syntax/docs →
consumer/browser confirmation. C would likewise require representation and JSX
policy decisions before its implementation; neither is a hidden task for #4006.

## A: exact accepted grammar and semantics

The outer argument remains an object literal, at the already-supported parameter
position. `createBoundary(deps)` stays rejected even if `deps` is a local const.
Existing member/destructuring forms, direct-call closure, binding identity,
parameter immutability, and same-definition-across-calls requirements remain.

A spread operand must be an identifier (existing TypeScript-only/parenthesis
unwrapping is allowed) resolved by SWC binding ID to a `const` object literal in
that same module. Function-local consts are allowed. Imported objects, parameter
objects, aliases, object expressions used directly as spread operands, call
results, members, and conditional operands are excluded. The source initializer
has **no spread**. One-level expansion is deliberate, not a recursive evaluator.

Apart from the accepted outer spreads, both the argument and each spread source
contain only shorthand or ordinary key/value data properties with static non-computed keys. Existing numeric and
bigint key handling remains; a nested object value for an unrelated settings
property is data, not another dependency spread to expand. Any getter, setter,
method, computed key, or unsupported spread anywhere in these top-level property
lists rejects the entire argument, even if a later property would hide it.

The source binding must have no assignment, update, delete, member mutation,
alias, destructuring read, member read, call argument, return, export, JSX spread,
array/positional spread, container storage, or other value escape. Its only value
references are operands of object-literal spreads. Multiple such spreads are
safe: each copies own data properties and cannot expose the source object itself.
Track binding IDs and references in nested closures; do not reuse the factory
parameter's static-member-read exemption for a spread source. Explicit export
syntax (declaration/default/specifier) and shorthand object references need
coverage because not all are expression-identifier visits. A conservative
rejection of a later write is required even if a particular runtime call happened
before it. This is a syntactic proof, not control-flow or temporal evaluation.

### Duplicate and override contract

1. A selected property explicitly declared twice in **one** object literal is
   rejected, even when the values are identical or another spread later overwrites
   it. This applies independently to the argument's explicit properties and to
   each source literal. Nonselected duplicate data keys remain allowed as in the
   old literal grammar. If another boundary selects that key, its proof rejects it.
2. A collision introduced **across** spreads, or between a spread and an explicit
   property, is accepted. Traverse the outer argument left to right; the last
   occurrence supplies the effective selected value. An empty or unrelated spread
   does not erase an earlier selected value. Repeating the same safe source is
   allowed.
3. Validate the entire object/source grammar first. Resolve and conditional-check
   only the effective selected value. Thus an earlier conditional value that is
   overwritten by a proven concrete value is accepted; a winning conditional
   still fails with reason 10. Unsupported source syntax is never hidden by an
   override. Function values overwritten away are not registered by this proof.
4. Reject a top-level key spelled `__proto__` in the argument or any spread source
   with reason 8, including shorthand and quoted spellings. `__proto__: value`
   sets a prototype rather than creating an ordinary own property. Shorthand can
   be own data at runtime, but it is deliberately excluded from this bounded
   grammar too; do not infer inherited properties, invoke getters, or model
   prototype construction. This also closes the selected-key ambiguity in the
   existing literal validator. Computed `['__proto__']` already fails reason 8.

With client functions `Counter` and `Other`, these are separate successful cases
(not calls to one factory with conflicting winners):

```tsx
const defaults = { Counter };
const override = { Counter: Other };
createBoundary({ ...defaults });                       // Counter
createBoundary({ ...defaults, Counter: Other });       // Other
createBoundary({ Counter: Other, ...defaults });       // Counter
createBoundary({ ...defaults, ...override });          // Other
createBoundary({ ...override, ...defaults });          // Counter
createBoundary({ ...defaults, ...defaults });          // Counter
```

Every direct call to one proven factory must still agree on the final **definition**,
not merely marker spelling. Two calls selecting `Counter` and `Other` fail the call-disagreement reason (12).
Alias/re-export identity uses the existing resolver; distinct functions with
the same name still reach the existing marker ambiguity error. New object facts
must never be interpreted as `Value::Function` themselves.

### Rejected shape and diagnostic matrix

For factory rows, retain the source-located outer diagnostic and
`target Target has unsupported initializer: ` prefix, the reason below (with real
names/locations), and `; see concepts/islands#boundary-discovery-and-migration`.
The generic rewrite suffix still follows. No new diagnostic family is needed.

| Rejected shape | Exact reason or existing diagnostic fragment |
| --- | --- |
| `createBoundary(deps)`; `createBoundary({ ...unknown, Counter })` | `call site at <loc> passes a non-literal argument` |
| `let deps = { Counter }`; imported/parameter/aliased sources; nested source spread; `...getDeps()`; `...{ Counter }` | `call site at <loc> passes a non-literal argument` |
| Any forbidden source reference/write/export, computed key, method, accessor, or `__proto__` key | `call site at <loc> passes a non-literal argument` |
| Selected duplicate explicit key in argument or source | `call site at <loc> passes a non-literal argument` |
| Safe expansion without the selected key | `property Counter missing at call site <loc>` |
| Winning `host.Counter ?? Counter`, ternary, or `Counter \|\| Other` | `call site at <loc> selects Counter conditionally (??, ternary, \|\|)` |
| Winning `host.Counter` with a runtime/opaque object receiver (not a namespace import) | `property Counter at <loc> is not a function binding` (append an existing resolver reason if present) |
| Two valid calls select different functions | `call sites disagree: <loc> passes Counter, <loc> passes Other` |
| Forward a factory parameter as `inner({ ...deps })` or `inner(deps)` | Existing `parameter deps is written or escapes at <loc>` takes precedence when that parameter's proof is demanded; A does not add interprocedural forwarding. |
| Store a direct-return fixed wrapper as `{ Panel: BarePanelIsland }`, or `ForwardBoundary` as `{ Island: ForwardBoundary }` | `boundary wrapper escapes into an opaque container` |
| Store it as `{ ForwardBoundary }` | `boundary wrapper escapes into an opaque object` |
| `<Island><PropsReceiver component={Counter} /></Island>` | SSR rejects `ZR_ISLAND_PROPS PropsReceiver: ZR_PROPS_FUNCTION at props.component`; both functions must be ordinary client exports and `PropsReceiver` must be the valid fixed target so this reaches transport validation. |

### Diagnostic order and performance budget

Preserve [BOUNDARY-DISCOVERY.md:414–440](../crates/zfb-islands/BOUNDARY-DISCOVERY.md)
reason order: default parameter → rest → parameter write/escape → factory escape →
no calls → too few arguments → positional spread → non-literal argument → missing
property → conditional selection → not-function binding → call disagreement.
Mutable targets and invalid binding patterns keep their existing earlier paths.
Process each reason category across the entire sorted call set (path, source span)
before the next category, not all reasons for one call at a time.

Classify source-shape/source-escape and duplicate-selected failures in category 8,
including duplicates found in a spread source. This intentionally fixes the old
edge where selected duplicates were checked beside missing properties: a duplicate
in any call must outrank a missing key in an earlier call. Add both call-order
permutations. Missing keys still outrank a conditional in another call; structural
failures still outrank a hidden/winning conditional. A collision across safe
spreads is not a duplicate failure.

Index object initializer and use/write/export facts once per module in
`NestedBindings`; cache validated source facts by `(module, SWC binding ID)`.
Use a shared indexed initializer/projection rather than walking/cloning the module
AST for every spread. Do not call `local_const_init` (R:2014–2055) per spread.
Cache selected-property lookup as needed so shared sources are not rescanned per
call or per target. Keep the factory proof memo key
`(factory Definition, parameter index, property)` and the separate target-write
validation cache; preserve demand-discovered-module invalidation.

Budget acceptance is deterministic work counts, not wall time:

- Preserve all existing budget assertions for their original inputs.
- Extend `factory_member_proof_is_memoized_across_200_call_sites` with 200 calls
  containing two reused spread sources, override directions that keep the same
  winner, and three repeated target resolutions: exactly one factory proof;
  existing resolver-operation `< 200 * 5` and demanded-resolution `< 200 * 4`
  bounds remain. For this larger input, indexed expression visits are bounded by
  `200 * 16 + 64` (two operand reads plus explicit property/argument work per
  call, including setup). Keep the original `< 200 * 10` variant too.
- Instrument or expose source-summary counts: each unique source validates once;
  a cached target resolution adds zero initializer/property scans. Include a
  shared source with 64 unrelated data properties to detect per-call source walks.
- Extend the packed-module linear AST test with 96 independent spread-bearing
  factory sites and same-spelled shadowed consts; at 192 sites expression work is
  at most twice the 96-site count plus 64 fixed-overhead visits. Source-summary
  evaluations are bounded by unique binding count, including rejected sources.
  Repeating the resolution pass adds no AST visits. Source property indexing is
  linear in total declared property count, not source-count × call-count.

These counters leave room for bounded syntax bookkeeping while detecting the
module-walk/cache regression. If the implementation cannot meet them, report the
measured operation and resolve the design before relaxing the budget.

## D: migration semantics, maps, and identity

Move the host/default choice to a server composition site that can name both
concrete targets. Keep package feature settings in ordinary nested server
factories. Select **rendered boundary descriptions**, not a function passed into
Island. A minimal recipe has `createChrome(settings)` calling
`createBodyEnd(settings)`, whose returned server view gates rendering and forwards
`children` unchanged to `PanelSlot`. `PanelSlot` returns a Fragment with those
children. Neither factory owns a dynamic-target Island.

At the composition site, render `HostPanelIsland` when the host path is selected;
otherwise render `DefaultPanelIsland` unless package-default suppression applies.
Each is an explicit named server function returning a Fragment containing
`<Island when="load"><HostPanel /></Island>` or the fixed `DefaultPanel` equivalent.
Both clients are named exports of real `"use client"` files. The following server
shape fixes the composition contract (the fixture modules supply the named client
imports and ordinary `PanelSlot`):

```tsx
import { Island } from "@takazudo/zfb";
import { DefaultPanel } from "./default-panel";
import { HostPanel } from "./host-panel";

function DefaultPanelIsland() {
  return <><Island when="load"><DefaultPanel /></Island></>;
}
function HostPanelIsland() {
  return <><Island when="load"><HostPanel /></Island></>;
}
function PanelSlot({ children }) {
  return <>{children}</>;
}
function createBodyEnd(settings) {
  return function BodyEnd({ children }) {
    if (!settings.enabled) return null;
    return <PanelSlot>{children}</PanelSlot>;
  };
}
function createChrome(settings) {
  const BodyEnd = createBodyEnd(settings);
  return function Chrome({ children }) {
    return <BodyEnd>{children}</BodyEnd>;
  };
}
```

The page constructs `Chrome` with its settings, then renders this server-only
selection inside it. `hasHost` describes a concrete host implementation installed
at this composition site; it is not permission to forward an arbitrary function:

```tsx
<Chrome>
  {hasHost
    ? <HostPanelIsland />
    : suppressDefault ? null : <DefaultPanelIsland />}
</Chrome>;
```

JSON data may be passed to those clients; a component function or the server slot child never
becomes a client prop. This is how the package can retain its ordinary settings
logic while the caller owns the target boundary. Disabled settings prevent the
child description from being rendered: no Island marker, activation, or package
pre-hydration shim should appear for that feature.

For maps, rewrite bare-return fixed Island wrappers to the documented Fragment
shape. Where a generic forwarding Island entry is used, replace it with explicit
fixed-target entries such as `DefaultPanel: DefaultPanelIsland` and `HostPanel: HostPanelIsland`.
Use the documented **Fragment-wrapped** return shape from
[MDX components](../docs/src/content/docs/concepts/mdx-components.mdx#islands-constraint),
and ensure the map module is reachable from a page. R:2168–2230 and R:2338–2348
still reject a recognized boundary value escaping into a container. The existing
Fragment recipe exposes the concrete Island as its own scanned form and avoids
the generic forwarding-wrapper escape. Do not replace it with a bare-return
`return <Island>…</Island>` map wrapper or claim arbitrary fixed-wrapper maps work.
The map is server-only. Consuming it in a nested server view must render the fixed
entry; it must never be sent through island JSON props. Test both the old map
failure and the replacement's SSR and activation.

Identity proof is unchanged: R:3349–3358 keys registration by defining function;
client-route validation follows it, then marker-name collisions fail (R:3404).
[island-boundary.ts:29–48](../packages/zfb/src/island-boundary.ts) checks function
name/displayName, build metadata and membership. The renderer
[render-html.ts:610–633](../packages/zfb/src/zudo-react/render-html.ts) checks the
identity and normalizes props; `props-transport.ts` rejects functions. The new
spread proof must yield the same defining function that the runtime object would
select. D makes every mounted function independently visible at a static boundary.

A statically discovered but settings-disabled boundary may still have a registry
entry. Assert the exact global registry separately from the per-page emitted
markers; do not require dead-code elimination of disabled/default branches, and
do not mistake an empty page-marker set for an empty build-wide registry.

## Locked fixtures and behavioural matrix

All positive routes live under
`crates/zfb/tests/fixtures/scanner-boundary-acceptance/pages/`. Reuse the existing
client markers `FactoryCounter`, `DefaultPanel`, and `HostPanel` and their stateful
buttons. Put nested server factories and map recipes in shared fixture modules;
include the package half in `package-source/dist/` so the real `npm pack` consumer
covers local and packed composition. These are fixtures, not vendored consumer code.

| Fixture name / route directory | Selection and gate | Exact page markers | Interaction |
| --- | --- | --- | --- |
| `factory-static-spread` | Safe local const spread; literal factory control retained | `FactoryCounter` once | Factory counter 0 → 1 |
| `factory-composition-default` | Nested factories, settings enabled, no host | `DefaultPanel` once | Default panel 0 → 1 |
| `factory-composition-host-override` | Nested factories, settings enabled, host supplied | `HostPanel` once, no default | Host panel 0 → 1 |
| `factory-composition-settings-disabled` | Host supplied, feature setting false | none | Neither button nor activated boundary |
| `factory-composition-default-suppressed` | Feature enabled, default suppression true, no host | none | Neither button nor activated boundary |
| `factory-composition-host-with-default-suppressed` | Feature enabled, default suppression true, host supplied | `HostPanel` once, no default | Host panel 0 → 1 |
| `factory-map-default` | Fixed-target map rewrite through nested server view, enabled | `DefaultPanel` once | Default panel 0 → 1 |
| `factory-map-host-override` | Same map rewrite, host supplied, enabled | `HostPanel` once, no default | Host panel 0 → 1 |
| `factory-map-settings-disabled` | Same map rewrite, host supplied, feature disabled | none | Neither button nor activated boundary |

Keep existing `/` and `/host-override/` checks. No root count is silently replaced.
All new active rows assert `data-zfb-island-mounted`, `data-when="load"`, one build
identity matching the emitted registry, JSON-compatible `data-props`, successful
click/state update, and no console/page errors. Disabled rows assert zero markers,
zero mounted attributes, no feature buttons and no feature shim/activation side
effect. Export expected per-route marker sets/counts to `acceptance.json` for the
Chromium harness; preserve the exact build-wide registry checks.

Negative build fixtures retain names `factory-spread` (undeclared `defaults`),
`factory-nullish`, `factory-escape` and `dynamic-child`; add
`factory-spread-mutated`, `factory-spread-nested`, `factory-spread-proto`,
`factory-function-json-prop`, `factory-map-opaque` (bare-return fixed wrapper),
`factory-map-forwarding-opaque`, and `factory-map-opaque-shorthand`. Use the matrix's exact diagnostic reasons and
assert nonzero exit. Scanner-preflight failures assert no published islands.
The JSON-prop failure is an SSR error after scanner success: assert no successful
page publication, without claiming it must happen before asset generation.

Unit coverage additionally locks both override directions, source/source and
explicit/source collisions, selected duplicates in both kinds of literal,
nonselected duplicates, omitted selected keys, overridden and winning conditionals,
unknown/imported/parameter/alias/call sources, every forbidden source use including
nested-closure writes and exports, computed keys/accessors/methods, shadowed
bindings, demand/cache invalidation, and cross-call diagnostic precedence.

## Documentation, verification, and ownership

#4007 updates EN and JA `concepts/islands.mdx` sections **Boundary discovery and
migration**, **Factory-provided targets**, and **Composition seam for customisable
package islands**; add a **Static local dependency spreads** subsection and a
**Nested factories, settings gates, and host overrides** subsection. Update both
`guides/troubleshooting.mdx` diagnostic tables and the `concepts/mdx-components.mdx`
**Islands constraint** section to distinguish the preserved Fragment recipe from
rejected bare-return fixed and generic forwarding maps. Both locales must cite the exact fixture
names above, explain override/duplicate rules, and explicitly name the downstream
public seam migration. Use fresh-consumer `@takazudo/zfb` specifiers; aliases may
link the existing #3932 parity note. No scanner or fixture changes in docs' ownership.

Follow [CLAUDE.md development policy](../CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions).
Integration parent is `base/sweep-261007-2`; all topic work first enters
`base/sweep-261007-2-factory-target-contract`. The manager owns CI triage and broad
verification. Scanner child: warm, cheap islands unit tests/fmt/scoped clippy and
fixture authoring. Docs child: `pnpm docs:check`. Do not make a cold compile a child
completion requirement. Confirm runs the whole Unix acceptance binary (cases are
not independently selectable), Chromium, identity integration tests, strict docs
build/emitted HTML, and no-V8 check through the manager's warm verification lane.
Heavy commands use `bash $HOME/.codex/scripts/heavy-guard.sh -- <command>`.

The confirm evidence table records tested SHA, command, duration, exit/status,
expected diagnostic/markers and actual evidence. The direct, plain literal,
static spread, conditional, composition and opaque-map controls all use the binary
from that SHA. Pending, deferred, cancelled, skipped and pass-on-retry are not
passes. A platform/environment failure gets the repository-prescribed baseline
or deferred-verification treatment; a real assertion failure is fixed. The decision
itself runs only scoped Markdown validation, reference/fixture-name consistency,
and a foreground in-session review; no production behaviour is claimed here.
