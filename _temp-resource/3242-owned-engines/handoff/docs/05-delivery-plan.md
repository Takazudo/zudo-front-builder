# 05 — Delivery plan for the local agent

## Working approach

Build narrow vertical slices and migrate the owner's consumers directly. Use the existing engines only as temporary development/reference paths where helpful. Do not build a general compatibility layer, an alternate bundler, or a plugin ecosystem.

The phase IDs below are planning labels, not existing issues, branches, commands, or implemented features. The machine-readable acceptance cases refer to these IDs. Follow the current repository's worktree, testing, and release instructions; this handoff does not prescribe a new branching system.

## Phase P0 — Reconcile the local baseline

**Goal:** establish what is actually present before changing the engine contracts.

Read applicable instructions and inspect `git status` and HEAD. Use `rg` to locate current CSS-engine construction, Tailwind extraction, framework enums, generated mount glue, JSX aliases, island wrappers, and config schemas. Compare relevant areas with the reviewed snapshot; do not reset or revert local work.

Locate real downstream sources. Start with zfb's own docs and the actual zudo-doc package/version they consume. Expand to zudo-sg or zzmod when their source is available and relevant. Record source revisions and execution environment. Avoid treating every repository in the owner's ecosystem as automatically part of the first rollout.

Deliver:

- A baseline note with current revisions, supported build modes, representative pages, and available browser/toolchain environment.
- A consumer inventory: utility/directive uses, CSS assets, hooks/context/refs/third-party component dependencies, islands, forms, conditional regions, lists, and client-navigation behavior.
- A local decision ledger that identifies which proposed defaults were adopted or refined.
- The smallest real consumer slice for W1/W2 and a runtime specimen for R1/R2.

**Exit:** real requirements and current integration paths are identified. Missing access is documented without invented class counts, hook counts, timings, or coverage claims.

## Phase W0 — Ratify the zudo-wind v1 behavior

Translate the inventory into an explicit catalog and migration table. Choose token input representation, numeric scales, variants, reset, arbitrary values, layering, and conflict order. The defaults in the wind spec can be adopted directly when they fit. Do not ask the owner again about compatibility.

For each unsupported current expression choose an owned utility, authored CSS, or a deliberate consumer change. Identify reset-dependent components and the CSS import/asset behavior the migration must preserve.

**Exit:** one versioned specification and a small set of high-value expected-behavior fixtures. No universal Tailwind parity target.

## Phase W1 — Pure compiler vertical slice

Implement the native library from explicit candidate/token inputs before adding file scanning. Start with enough utilities to style the chosen page. Candidate parsing, value/token validation, variants, sorting, and CSS emission should be independently callable. Expose structured diagnostics and explanation metadata.

Use the existing CSS parser/processor as appropriate. A utility catalog can generate documentation; build a minimal explanation path from the same metadata. The exact proposed `zfb wind explain` CLI can follow once the library has a useful explanation result.

**Exit:** supported expressions produce deterministic CSS and unsupported explicit inputs produce useful errors. Padding shorthand/axis/side conflict fixtures pass with class order permuted. This is a compiler milestone, not a migrated site.

## Phase W2 — Source and asset integration

Connect the library to a zfb-owned authoritative source plan. Implement complete-candidate extraction, explicit generated manifests, candidate replacement/removal, and dependency invalidation. Reuse authored CSS, imports, CSS Modules, assets, hashing, and publication paths where appropriate; extend lossy engine interfaces rather than inventing global mutable side channels.

Verify original stylesheet identity through imported CSS and package-relative `url()` references. A utility engine replacement cannot leave the old Tailwind compiler responsible for required import/asset behavior. Distinguish ordinary authored classes from invalid utility expressions.

**Exit:** the real slice builds and rebuilds with the new engine; required fonts/images work; clean and warmed builds agree; deleted class contributions disappear at the correct time. The new path invokes no Tailwind executable.

## Phase W3 — Chosen consumer migration

Migrate tokens, directives, utilities, and reset assumptions in the selected consumer. Remove obsolete Tailwind imports/config entries in that consumer. Make unsupported expressions explicit; do not silently broaden the engine to all upstream features.

Review representative states at relevant viewports: normal content, focus, expanded controls, errors, dark mode, and client-only branches. Integrate consumer documentation with the catalog. Record intended appearance changes separately from regressions.

**Exit:** the selected consumer works under the owned wind contract with required styling and assets accounted for. Do not claim all other consumers migrated.

## Phase W4 — Wind packaging cleanup

After selected rollout requirements pass, remove unused Tailwind binary embedding, extraction, warm-up, version pins, fetch scripts, and temporary CSS-input machinery. Search for all production call sites before removal. Update applicable CLI help, config types, templates, and installation documentation.

A development-only pinned Tailwind reference may remain if it has a concrete purpose and does not reenter the production path. Its fixture comparisons cover only intentionally inherited behavior.

**Exit:** an installed zfb build can execute the supported wind workflow without a Tailwind executable or per-project Tailwind dependency. Authored-CSS-only behavior still works where retained. esbuild still serves its existing roles.

## Phase R0 — Fix the runtime specimen and contract

Select real UI requirements from the inventory. The minimum specimen should contain static composition, a reactive counter, tabs, an editable input, and a removable subscription-owning widget. It should use CSS already available; runtime progress must not depend on expanding wind.

Adopt/refine the setup-once contract, reactive JSX rules, activation scopes, form policy, marker format, mismatch behavior, and server/client version checks. Decide whether the chosen consumer requires structural regions and keyed lists before calling its migration complete.

**Exit:** expected instance lifetime, data crossing, form behavior, and disposal are written down. No React compatibility layer is specified.

## Phase R1 — JSX descriptions, reactivity, and server HTML

Implement a small JSX runtime using the standard transform, with matching JSX types. Implement signal/computed values and scope ownership under the defined scheduling rules. Produce correct escaped HTML and deterministic boundaries from the same description model the browser hydrator will consume.

Server rendering must not run activation callbacks or create lasting browser subscriptions. Per-render/per-island state must not leak across requests. Only supported serializable initial data crosses the island boundary; functions are recreated from the client module.

**Exit:** deterministic HTML, adjacent/empty text boundaries, attribute/property rules, and isolated render state are verified. No hydration claim yet.

## Phase R2 — Browser adoption and lifetime

Implement a validation/adoption pass, binding attachment, native events, activation, and idempotent disposal. Preserve existing nodes. Validate structural and transport compatibility before activating effects. A mismatch must produce the specified diagnostic; a silent remount cannot satisfy the gate.

Implement uncontrolled and explicit model-bound form policies, prehydration edit handling, and IME behavior. Include cleanup when an owned region disappears. A canceled deferred island must never activate against removed DOM.

**Exit:** the real-browser specimen proves node identity, updates, form state, and cleanup. Test the emitted client bundle loaded against the emitted server HTML, not only handwritten DOM or source-level helper calls.

## Phase R3 — Structural operations required by the consumer

If inventory shows conditional mounting, keyed lists, dynamic search results, or similar needs, implement them as explicit bounded operations. Specify node identity, key uniqueness, state retention, disposal, and hydration markers. Add a focused-input reordering case for keyed lists.

This phase is legitimately unnecessary for a consumer using only static structure with reactive visibility. Record that actual scope. It cannot be skipped while claiming a list-heavy consumer is migrated.

**Exit:** the chosen consumer's necessary structural operations exist and pass behavioral cases. Unsupported operations remain explicit.

## Phase R4 — zfb and consumer integration

Wire the runtime into current framework selection, JSX transforms/types, SSR rendering, VNode-dependent wrappers, shared/per-island emitted entry modules, module resolution, and installed-package embedding. The adapter shim is only one of these paths.

Map runtime disposal to zfb's island lifecycle. Preserve the established ownership distinction between a removed island and one intentionally persisted through navigation; changed-props remount rules must be deliberate. Prevent duplicate mount and runtime singleton duplication.

Migrate one selected real consumer's component/state code. Hooks may become local signals, computed bindings, activation callbacks, or ordinary modules. The migration should follow behavior, not mechanical renaming. Third-party Preact/React components need replacement or isolated continued use with an explicitly chosen boundary; no automatic compatibility promise.

**Exit:** generated SSR and client output use the new runtime coherently, actual interactions work, and navigation releases/preserves state as specified. Update templates and runtime documentation.

## Phase X1 — Combined rollout and evidence

Build the selected consumer with both new engines through the normal installed zfb workflow. Confirm esbuild remains responsible for bundling and no accidental alternate compiler was introduced. Remove temporary compatibility/legacy paths once their named migration conditions pass; inspect reachability before deleting embedded packages.

Record current same-environment measurements: cold build, warm edits (CSS, TSX, content), CSS output, client output, and relevant memory observations. Separate build orchestration from engine cost. Historical issue measurements in the sources document are context, not the baseline.

Update user-facing docs, configuration references, examples, and migration notes. Run applicable repository gates once the change is ready, following current instructions. Release/publishing follows the local session's authorized workflow rather than an assumption that generating this handoff published anything.

**Exit:** selected consumer and supported distribution path pass meaningful gates; remaining migrations and platform gaps are accurately named. The final report distinguishes implemented, verified, deliberately unsupported, and unfinished work.

## Parallel work without interface drift

After P0, utility catalog/compiler work and renderer specification work can proceed independently. CSS asset/source integration depends on a settled compiler result contract. Runtime hydration depends on the SSR boundary format and scope model. Consumer migration depends on the relevant supported behaviors, not merely package existence.

Assign one owner to each shared contract: wind compile result/source plan; runtime JSX/value types; SSR/DOM markers; zfb island mount/dispose bridge. Merge those contracts before distributing dependent implementation tasks. Use existing repository practices for heavy build serialization and worktree resource use.

## Reasons to refine scope, not fake completion

- An actual consumer relies on an omitted CSS feature: migrate it to authored CSS or add a documented rule.
- A real widget needs dynamic lists: implement the bounded list operation before switching that widget.
- Upstream version changes: preserve the owned spec; investigate only source paths relevant to the implementation.
- An environment prevents a required browser/distribution check: report the exact unverified condition; passing a smaller check does not prove it.
- A prototype shows no speedup: evaluate simpler integration and owned semantics on their merits; do not invent a performance claim.
