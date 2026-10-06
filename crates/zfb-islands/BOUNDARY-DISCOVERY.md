# Island boundary discovery contract

Accepted implementation contract for [#3506](https://github.com/Takazudo/zudo-front-builder/issues/3506),
implemented by [#3507](https://github.com/Takazudo/zudo-front-builder/issues/3507) and
verified by [#3508](https://github.com/Takazudo/zudo-front-builder/issues/3508).
Source problem: [#3384](https://github.com/Takazudo/zudo-front-builder/issues/3384).
This document is the normative scanner contract. The implementation landed in
#3507; #3508 adds real production and packed-consumer acceptance, browser
hydration checks, and migration guidance.

## 1. Register boundary targets; preserve the module graph

A `"use client"` directive marks a client module. It does **not** register every
export. Register a function only when a statically resolved SDK `Island` boundary
uses it as its single component child in the reachable page graph. The target
function, not a helper, export alias, or boundary wrapper, owns the public marker.

Reachability remains the existing route/module import closure, including injected
package routes, supported literal dynamic imports, aliases, and virtual-module
bridges. This is static discovery, not execution of the application: a concrete
boundary site in a reachable module counts even inside an unexecuted branch or
function body. No arbitrary call-graph or dead-code execution proof is required.
An exported function without a boundary use never registers merely because it
is exported, capitalized, returns JSX, or looks callable.

Maintain two separate results:

- **Module facts and dependency closure:** all existing import, side-effect,
  raw-resource, worker, glob, workspace-package, and watch/invalidation facts.
  Discovery of these facts must not depend on whether an export registers.
- **Validated target registry:** only resolved boundary targets, deduplicated by
  defining binding identity and checked for public-marker ambiguity.

Keep the current client-module closure used by `ScanMeta` independently of the
smaller registry. In particular, do not replace its directive-bearing module
roots with registered target paths. Existing guards/staging must still see
reachable helper-only client modules and their resources. Bundling follows the
real imports of registered functions and independently configured client-script
entries; ordinary tree shaking may drop unused exports. Retaining graph facts
is not a requirement to force every scanned server/helper module into browser JS.
A helper called by a live island, its transitive imports, and required module
side effects must survive; an unused helper must occupy no registry slot.

## 2. Preserve these repository seams

| Existing surface | Required implementation change or invariant |
| --- | --- |
| `src/scanner.rs`: `scan_islands_with_meta_and_first_party_root`, `exported_island_records` | Replace export enumeration as registration authority with binding-aware boundary discovery; preserve directive parsing and graph metadata. |
| `src/scanner.rs`: `Resolver`, `FsResolver`, `resolve_module_edge`, SWC binding resolution used by worker scanning | Reuse resolution and lexical binding facilities. Resolve demanded package bindings beyond traversal shortcuts when necessary, without globally scanning all dependencies. |
| `src/bundler.rs`: `Island` | Keep export access, public marker, and defining binding identity distinct. Today equality is only `(source_path, component_name)`; that is insufficient for aliases. |
| `src/manifest.rs`: `Manifest::from_islands`, `is_same_package_duplicate` | Validate identities before flattening to `marker -> path`; remove package membership/byte-copy suppression from registration validation. |
| `src/esbuild.rs`: `render_shared_bundle_entry_source_with_build` | Emit one registration per validated identity/marker using the exact selected export. Do not recover a missing named export via `ns.default`. |
| `../zfb/src/commands/build.rs`: islands scan, manifest validation, shadow remap | Propagate unsupported registration as a production error, validate before emission, preserve source identity through staging, and feed the validated set to bundling. |
| `../zfb/src/commands/bundler_input.rs`: scanner preflight and `zudo_react_island_names` | Validate before reducing targets to a `BTreeSet` of names; that reduction currently hides distinct bindings with one marker. |
| `../zfb-build/src/bundler.rs`: `zudo_react_island_names`, `zudo-react-build.mjs`, `zudo_react_build_token_with_inputs_and_output` | SSR metadata and client registration must derive from the same validated target set and build inputs. Audit all scan consumers, including dev/rebuild and worker-only SSR paths. |
| `../../packages/zfb/src/island-boundary.ts`, `zudo-react/render-html.ts`, `zudo-react/hydrate.ts`, `runtime.ts` | Preserve function/display-name, component/build, parser, props, and hydration checks. No new public registration API. |

Rust type names are implementation choices. The following conceptual fields are
required information, whether carried directly on `Island` or in an internal
validated record: defining module/binding, importable module/export, public marker,
client eligibility provenance, boundary sites, and alias/re-export trace.

## 3. Resolve lexical bindings, exports, and package identity

Use SWC lexical binding identities (including scope), not identifier text. A local
parameter named `Island`, `h`, or `Counter` must not match the imported binding it
shadows. Ignore type-only imports/exports. Erase parentheses and TypeScript-only
assertions/non-null/satisfies wrappers when resolving a value.

Recognize the SDK `Island` export from `@takazudo/zfb` and the owned description
factories `h` from `@takazudo/zfb/zudo-react`, `jsx`/`jsxs` from its `jsx-runtime`,
and `jsxDEV` from its `jsx-dev-runtime`. Treat these as known terminal exports;
do not inspect SDK internals as application boundary wrappers. Follow their
named aliases, local immutable aliases, named re-exports, and barrel re-exports.
A similarly spelled export from another library is not an SDK boundary/factory.
There is no SDK default `Island` export, but a user barrel may re-export its
named `Island` as that barrel's default.

Required value-resolution forms for both boundary and component bindings:

- Named/default imports and local aliases: `import Local from './counter'`,
  `import { Counter as Local } from './counter'`, `const Alias = Local`.
- Local export lists, default identifier exports, named function exports,
  named or direct anonymous default functions/arrows, and immutable function/arrow bindings.
- `export { Counter as Renamed } from './counter'`,
  `export { default as Counter } from './counter'`, and multi-hop barrels.
- `export *`: explicit exports take precedence; `default` is excluded; two
  distinct providers of the requested export are ambiguous. Multiple paths to
  the same defining binding are one provider. Cycles terminate with a worklist
  or memoized resolution state, without choosing whichever edge was seen first.
- Namespace imports/re-exports with a static member (`UI.Counter`, `SDK.Island`,
  `UI['Counter']`). The namespace object itself is never a component. Dynamic
  computed members demanded by a boundary are unsupported.
- Package export/subpath resolution, tsconfig/plugin aliases, installed tarballs,
  and workspace links use the same effective module resolution as the build.
  An unresolved demanded binding is an error, even if ordinary import walking
  previously skipped that bare package edge.

A target must have an importable runtime export through a `"use client"` module.
That module may export its own function or re-export a function from an underlying
module without the directive. Resolve to the underlying definition for identity,
but retain an accessible export route through the client module for bundling.
A boundary wrapper need not carry `"use client"`; its child must meet this rule.
An unexported function in a page is unsupported: move/export it from a client
module. Do not synthesize public exports or execute a factory to obtain one.

There is one bounded exception to the ordinary component-value grammar:
parameter-member targets are allowed at any function-body depth when the
function is statically called. Accept a `const` member binding such as
`const Target = deps.Counter` or
`const Target = deps["Counter"]`, a `const` object pattern such as
`const { Counter: Target } = deps` or `const { Counter } = deps`, and parameter
destructuring such as `function F({ Counter: Target })` or
`function F({ Counter })`. The parameter must be a plain identifier without a
default, and the member key must be static. Destructuring does not support rest,
defaults, or nested patterns. The parameter and target must satisfy the
read-only and closed-call-site proof in section 5. `let` and `var` retain the
existing `mutable target binding` rejection. This proof is syntactic: it does
not execute `F`, invoke a factory to obtain a component, or synthesize exports.
Calls such as `factory()` that return a component remain unsupported.

Canonical identity is `(logical defining ModuleId, defining BindingId)`:

- Resolve all imports and re-exports to the declaration that creates the
  function object; eliminate immutable identifier aliases on the way. Export
  spellings and boundary-local names are access routes, not identities.
- Canonicalize physical source paths (with normalized resolver IDs for in-memory
  sources). Preserve plugin virtual IDs as logical bridge IDs, never as temporary
  filenames. A direct virtual client target remains unsupported with a source-site
  diagnostic; a virtual barrel may lead to a real client target.
- Carry trusted source-to-shadow remaps supplied by zfb as provenance, not new
  identities. Do not infer equivalence from matching file stems, bytes, package
  root/name/version, function bodies, or display names. Different installed copies
  and separately authored source/dist definitions are distinct unless resolution
  or an explicit trusted provenance mapping proves the same definition. A symlink
  to the same source definition is one identity; emitter selection must use that
  canonical route so `preserve-symlinks` cannot instantiate two registry entries.
- SWC context/span IDs may be used inside one scan; deterministic output must not
  serialize process-global allocator IDs or random shadow paths. Sort by logical
  module, stable declaration location/name, and chosen export route.

`const Alias = Original; export { Original, Alias }` and default/named re-exports
of `Original` are equivalent. `const Alias = props => Original(props)` creates a
new function and is not equivalent. `memo(Original)`, `factory()`, conditional
bindings, mutable/reassigned bindings, and dynamic imports as component values
are not proof of same-function identity; demanded targets using these forms fail
as unsupported. Do not transfer the current `marker_from_default_expr` heuristic
that guesses identity from a factory call's identifier argument.

## 4. Recognize boundaries in source and shipped JS

These forms are mandatory, with aliases resolved as above:

```tsx
import { Island as Boundary } from '@takazudo/zfb';
import { h as node } from '@takazudo/zfb/zudo-react';
import CounterAlias from './counter';

<Boundary when="load"><CounterAlias count={1} /></Boundary>;
<Boundary children={<CounterAlias count={1} />} />;
Boundary({ when: 'load', children: node(CounterAlias, { count: 1 }) });
node(Boundary, { children: node(CounterAlias, { count: 1 }) });
```

Also recognize the equivalent `jsx`, `jsxs`, and `jsxDEV` calls in packed JS;
those factories may describe the boundary itself and/or its child. An emitted
`(0, importedFactory)(...)` call is equivalent only when the callee's sequence
prefix is the literal `0` and its final expression resolves to that factory.
Do not interpret arbitrary calls as description construction. For `h`, explicit
third-and-later child arguments override `props.children`, exactly as
`zudo-react/index.ts` implements; unknown child argument spreads are unsupported.

Extract the one effective child using JS property/JSX override order. Explicit
final `children` may override an earlier unknown spread; a later unknown spread
that may replace `children` is unsupported. A single known description can pass
through an immutable local alias. Normalize statically known child arrays and
JSX whitespace/null/boolean placeholders consistently with SDK `singleChild`;
zero or multiple component children are invalid. A DOM element or Fragment is
not the function component target. Do not select a descendant component from
inside a DOM/Fragment wrapper. Component props remain runtime values; the scanner
need not evaluate them. Existing serialization and nonempty-child checks on the
component's own props remain the SDK/renderer responsibility.

A conditional around two separate concrete boundaries can register both. A
conditional/dynamic expression selecting the child of one boundary is unsupported
in this finite contract; rewrite it as separate static boundaries. SDK boundary
options (`when`, `ssrFallback`, persistence) do not change target identity.

## 5. Summarize wrappers with a finite domain

A wrapper is a function whose return is an SDK boundary or a call/description of
a summarized wrapper. This is separate from a component factory that returns a
new function. Summaries are:

| Summary | Meaning at a use site |
| --- | --- |
| `Fixed(TargetRef)` | The boundary always receives one statically identified component description. Register that child, never the wrapper. |
| `ForwardChild` | The boundary receives the wrapper's single `children` value unchanged. Substitute the concrete child supplied at each call/JSX site. |
| `Unsupported(reason, origin)` | Boundary-related flow exists but cannot be proven by these rules. Emit an actionable error at the demanded use and origin. |

Required examples (the `Counter` export is eligible under section 3):

```tsx
function CounterBoundary(props) {
  return <Island><Counter count={props.count} /></Island>;
}
function LazyBoundary({ children }) {
  return <Island when="idle">{children}</Island>;
}
const PassBoundary = props => Island({ children: props.children, when: 'load' });

<CounterBoundary count={1} />;
<LazyBoundary><Counter count={2} /></LazyBoundary>;
PassBoundary({ children: h(Counter, { count: 3 }) });
```

Support a named function or immutable arrow/function binding with one expression
return (expression arrow or one top-level return), immutable local aliases, and
`props.children`/destructured `children` (including a renamed destructuring binding).
`Island(props)` or `<Island {...props} />` is `ForwardChild` when the same incoming
props are passed unchanged. Unrelated prop calculations do not require evaluation,
but writes, escapes, or unknown calls affecting the tracked child/props break
proof. Do not borrow returns from nested functions. Multiple returns, conditional
child selection, loops that compute the child, rest-child transformations, cloning,
and arbitrary higher-order factories are outside this grammar.

Compose summaries through imports/re-exports and wrapper chains until a finite
fixpoint: `ForwardChild` preserves a supplied target, and `Fixed(T)` supplies `T`.
A recursive cycle with no concrete summary fails on use; do not impose an arbitrary
hop limit that silently truncates discovery. At each forwarding call site resolve
its single child to a concrete target or a containing forwarding parameter.
A forwarding wrapper definition alone supplies no target; an unused definition
must not cause a missing-child error. A fixed concrete SDK boundary is still a
static boundary site under section 1.

Keep unresolved boundary obligations while summarizing. A wrapper with
`Island({ children: chooseChild(props) })` must not become an ordinary ignored
component. Reject an unsupported concrete SDK boundary, a use of an unsupported
boundary wrapper, or escape of a known SDK boundary/forwarding-wrapper value into
an opaque call/container that prevents finding its uses. Ordinary helper calls
and unrelated components do not become errors because their bodies are opaque.
A completely unavailable third-party implementation cannot be inferred to contain
an SDK call; when it is demanded as a boundary target or proven wrapper dependency,
report the unresolved source/export instead of fabricating a registration.

### Bounded factory-member targets

A parameter-member target is a component value selected from a function parameter,
not the result of executing a component factory. The accepted binding forms are
listed in section 3. The plain identifier parameter belongs to the function
owning the binding (`F`, or its enclosing function/ancestor as applicable); take
the proof on `F`'s call sites. It is not a rest parameter and has no default
value. TypeScript-only wrappers are unwrapped. Only `const` target and
object-pattern bindings are accepted; `let` and `var` keep the existing
`mutable target binding` diagnostic.

The read-only parameter rule is conservative. Every reference to the parameter
inside the owning function must be either the object of a static member read or
the initializer of a `const` object-pattern declaration. Assignment to the
parameter or one of its properties, `delete`, passing it, aliasing it, returning
it, spreading it, or any other use fails, including a reference from a nested
closure. A destructured target binding is never assigned. Ordinary reads of the
bound target such as `!Target`, `typeof Target`, or passing `Target` to a
non-Island helper do not affect registration; only its use as an `<Island>` child
does.

The function owning the parameter must resolve to a `FunctionValue`: a named
top-level or nested function, or a `const F = function|arrow` binding. Its call
site set is closed across the scanned module set, including import/re-export
chains that `resolve_export` can follow. Every reference to the function binding
must be the callee of a direct call (`F(...)`, `(0, F)(...)`, or `ns.F(...)`).
Passing, storing, returning, binding, applying, or using `F` as JSX is an escape.
An unfollowable export also fails the proof. The closure claim is limited to the
modules in the scanner's discovered graph; it does not claim to account for
runtime-generated or otherwise undiscovered callers.

For parameter index `i` and static property `<prop>`, the call-site set must be
non-empty. No call may spread positional arguments at or before `i`; every call
must pass at `i` a plain object literal with no spread, computed key, method, or
getter anywhere in the literal. `<prop>` must appear exactly once, as a shorthand
or explicit static property. Other plain properties in the object are allowed.
Each selected value must resolve to a `Value::Function`, and all calls must
resolve it to the same `Definition`. A target registers as if that same function
were the direct `<Island>` child; its identity, marker, route, and deduplication
are unchanged.

Collect `calls_by_callee: BTreeMap<Definition, Vec<CallSite>>` incrementally as
each newly scanned module is visited once. Defer proofs until the module set is
stable, or invalidate and re-prove an earlier result when a later module adds a
call or reference to that function. Memoise proofs by `(F Definition, i,
<prop>)`; budget tests count AST visits and resolver/proof operations.

## 6. Keep public marker and build identity compatible

Public identity stays `{ component: marker_name, build: build_token }`. Keep
`data-zfb-island`, `data-zfb-island-skip-ssr`, payload identity, and the flat
manifest JSON format. Canonical binding IDs are internal; do not replace public
marker strings with paths/hashes or introduce a registration API in this work.

For supported function declarations/expressions, derive the marker from the
actual function's name: a named function expression keeps its inner function
name; an anonymous arrow/function assigned to a `const` gets that binding's
inferred name. Alias/export spelling does not rename the function. For example,
`export default function Counter(){}` imported as `Local`, or re-exported as
`Renamed`, still registers `Counter`. `const Public = function Inner(){}` has
marker `Inner`. A direct anonymous default function/arrow has the language-inferred
name `default`; preserve that existing marker and identify its declaration with a
synthetic default binding. Production fixtures must prove both compilers preserve
that name. Two distinct such targets therefore genuinely conflict on `default`;
recommend explicitly named functions. An opaque default expression/factory is not
licensed to fall back to `default`. A function whose runtime name is actually empty
and class targets are unsupported; give a named-function migration diagnostic
instead of inventing a filename-based identity.

Retain SDK validation: a function must have a nonempty name; `displayName` may be
absent or equal to that name, but must not rename it. A known conflicting literal
assignment can fail during scanning; otherwise existing SSR/runtime checks must
still fail closed. Do not suggest setting a different displayName to repair a
collision. Do not infer an owned marker from a textual
`renderSsrSkipPlaceholder('Name', ...)` call: the current SDK has no such export;
legacy fixtures must be migrated to actual owned boundaries, not kept as a hidden
registration API.

Both SSR (`../zfb-build/src/bundler.rs`) and client (`src/esbuild.rs`) currently
use `--keep-names`. Preserve these settings and production minification behavior.
The browser emitter must validate the exact picked function against the marker
(and must not accept a conflicting displayName that SSR rejects). A missing
export or identity mismatch must not overwrite another registry entry. A defensive
runtime duplicate guard may reject a different function under an existing marker;
it cannot replace build-time canonical identity validation.

One validated target set supplies both `globalThis.__zfb.zudoReactIslands` and
client entries. The SSR preflight in `../zfb/src/commands/bundler_input.rs` currently
collects names into a `BTreeSet`; validate before that operation loses binding
identity, including when the browser asset emitter has not run yet. Separate internal scans are allowed only when they share the
same discovery/validation implementation, resolution inputs, and deterministic
results. Preserve the existing 16-character lowercase-hex build token and the
shared token-generation inputs for SSR/client, including plugin aliases, virtual
modules, output directory and defines. Test minified output by executing/rendering
it, not only checking a generated string or compiler flag.

[#3383](https://github.com/Takazudo/zudo-front-builder/issues/3383) is tracked by
[#3460](https://github.com/Takazudo/zudo-front-builder/issues/3460) and #3468–#3472.
Its size work must consume this identity contract: measure the smaller registry
first, and retain name preservation unless a separate coordinated change provides
an explicit compatibility mapping across scanner, SSR, emitted registry and
hydration. Namespace-import/tree-shaking optimization and new size budgets are
not part of #3507/#3508. No keep-names reduction is authorized by this contract.

## 7. Validate all targets before flattening or emitting

Group discovered targets by canonical defining binding and deduplicate alias
routes first. Choose an importable route deterministically, preserving the
necessary client-module side effects; retain other required import edges in the
module graph. Then group by public marker:

- One canonical binding: exactly one registry entry, however many boundary sites,
  default/named exports, symlinks, or barrels reach it.
- Different canonical bindings sharing a marker: hard ambiguity error, including
  two definitions in one file, two files in one package, package versus consumer,
  and source/dist copies without trusted equivalence. No first-wins/last-wins,
  same-package exemption, byte-copy exception, or filesystem-order selection.

Validate the full registry before writing the flat manifest, SSR allowed names,
client entry, or successful production output. Every shipping bundler entry path
must consume validated targets or perform equivalent validation; checking only
the CLI manifest and then emitting an unfiltered raw scanner vector is insufficient.
`Manifest::from_islands` must not erase defining binding information before this
check. The public JSON may stay flat after validation.

## 8. Fail actionably and define the migration

Add a typed unsupported-registration scan error (and an ambiguity error at the
validation seam) containing:

- Boundary use file, line and column; wrapper origin when relevant.
- Requested binding/export, resolved module and alias/re-export chain, where known.
- The unsupported expression or ambiguity reason, and a concrete supported rewrite.
- For collisions, both definition locations and representative boundary uses;
  package/subpath context for dependency-owned targets.

Example diagnostic content:

```text
unsupported island registration at pages/home.tsx:12:3
  LazyBoundary -> components/boundary.tsx:4:10 -> SDK Island
  children: chooseChild(props) is dynamic
  Export a named function from a "use client" module and pass <Counter />
  (or h(Counter, props)) directly; for conditional targets use separate boundaries.
```

For a genuine collision, rename a defining component function or correct a wrong
import. Dependency owners can expose distinct named functions/re-exports; consumers
can update/fix the package or avoid rendering the conflicting target. Renaming
unused helpers is never required. Source/dist duplication is repaired by resolving
to one exported definition or supplying trusted build provenance, not by restoring
the package-wide exemption.

**Production must fail nonzero.** The current `build.rs` generic `ScanError` arm
warns and skips island emission. Route new registration/identity errors through
its hard-error policy. The SSR preflight in `bundler_input.rs` already propagates
scan errors with `?`; preserve that behavior and add ambiguity validation before
its name-set projection. Dev
may keep the server alive and report the source-site diagnostic, but it must not
publish a partial registry or mark the invalid rebundle successful. Keeping a last
known-good artifact is acceptable only with a visible failed rebuild state.
Existing SDK runtime errors remain a backstop for values that cannot be validated
statically; no failed scan may be presented as a valid empty-islands result.

For a factory-member target, keep the diagnostic prefix
`target Target has unsupported initializer`, append the first matching reason
below, then the standard rewrite suffix and a link to
[`Composition seam for customisable package islands`](../../docs/src/content/docs/concepts/islands.mdx#composition-seam-for-customisable-package-islands).
The function and property placeholders contain the actual binding names; `<loc>`
is the source location. A `let` or `var` binding keeps its existing
`mutable target binding` reason before this proof is attempted. The ordered
factory-proof reasons are:

1. `parameter <name> has a default value`
2. `destructured parameter <name> uses a rest pattern`
3. `parameter <name> is written or escapes at <loc>`
4. `factory <F> escapes as a value at <loc>`
5. `factory <F> has no call site in the scanned modules`
6. `call site at <loc> passes too few arguments`
7. `call site at <loc> spreads positional arguments`
8. `call site at <loc> passes a non-literal argument`
9. `property <prop> missing at call site <loc>`
10. `call site at <loc> selects <prop> conditionally (??, ternary, ||)`
11. `property <prop> at <loc> is not a function binding`
12. `call sites disagree: <loc> passes Counter, <loc> passes Other`

When resolving a selected value already yields an `Unsupported` result (for
example, the existing `memo(X)` reason), append that resolver reason to the
`not a function binding` diagnostic. Do not replace it with a guessed identity.
The page-level migration table lists the concrete rewrite for each reason.

## 9. Required acceptance evidence

#3507 owns focused AST/resolution/registry/entry tests and error propagation.
#3508 owns real production and packed-consumer acceptance plus EN/JA migration docs.
Every positive fixture must assert the exact marker set, not just successful exit.

| Case | Required result |
| --- | --- |
| Two local client modules each export unused `readState`; their default components are boundary children | Build passes; only the two component markers exist. |
| Live target calls an imported helper that itself imports code/resources | Executed browser behavior proves the helper survives; graph metadata/staging/watch edges remain present. |
| Unused helper-only client module and zero-boundary route | No helper registration; no false missing-target error; existing client-script/router behavior remains intact. |
| SDK alias, shadowed local names, type-only imports, unrelated library `Island` | Only genuine SDK boundary bindings count; no spelling-based false positives. |
| Named/default aliases, namespace members, explicit and star barrels, cycles | Correct defining binding and original function marker; same binding deduplicates; conflicting star exports fail when demanded. |
| JSX, direct `Island({children:h(...)})`, `h(Island, ...)`, packed `jsx/jsxs/jsxDEV` | Same target set and runtime behavior, including aliased factories. |
| Fixed/forwarding wrappers, nested wrapper chains, local/package call sites | Register concrete child only; forwarding definition without a use contributes no target. |
| Factory target, dynamic child, unresolved demanded import, opaque wrapper or escape | Nonzero production build with use-site/reason/rewrite; never warning plus successful missing bundle. |
| Accepted factory-member page | Build passes; supported `const` member, `const` object-pattern, and parameter-destructuring forms register the one function definition selected by every direct call-site literal. Other plain object properties and non-Island reads of the target are allowed. |
| Factory proof near misses | Positional spreads; object-literal spreads, computed keys, methods, or getters; conditional selection; missing call sites; escaping factories; default/rest/mutable bindings; parameter write/escape; too few arguments; missing properties; non-function values; and disagreeing call sites fail nonzero with the first applicable named reason and rewrite. |
| Two real targets with the same function name, including two bindings in one file | Hard ambiguity error before any registry can silently overwrite. |
| Installed tarball with duplicate helpers | Pass; helper markers absent; emitted registry has unique keys. |
| Installed tarball with two distinct actual same-name targets in the same package | Fail just as local collisions do; same-package bypass cannot suppress it. |
| One actual function via package re-exports plus a consumer's distinct target | One registration for the package function; both package and consumer targets hydrate. |
| Separate source/dist definitions or identical bytes without trusted mapping | Remain distinct; fail if actual targets share a marker. Proven source/shadow aliases deduplicate. |
| Named/anonymous literal default, inferred arrow name, named function expression and equal displayName under production minification | SSR marker = allowed-name metadata = emitted registry marker = hydrated identity; real interaction succeeds. |
| Conflicting displayName or missing/incorrect build token | Existing fail-closed identity checks remain; no silent renamed registration. |
| Invalid registration during dev rebuild and worker-only SSR preparation | Visible failure without partially updated registry/allowed names; valid recovery recomputes the same target set. |

Use current inherited code after earlier sweep work. Unit/type/lint checks run
directly; packed builds, Cargo builds, browser/e2e and other heavy suites use the
machine heavy guard and run serially. #3506 itself changes only this design contract
and downstream issue instructions; it does not run Cargo/browser acceptance or
claim that these fixtures already pass.
