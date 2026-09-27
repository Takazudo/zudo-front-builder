# zudo-react: JSX, explicit reactivity, and true hydration

## 1. Status and authority

This is an implementation handoff, dated 2026-09-28 in Japan. `zudo-react` is a working name for an original runtime. The name does not promise React or Preact compatibility. No runtime has been implemented or benchmarked in this handoff.

**User decisions:** compatibility may be dropped completely; hooks are unnecessary; JSX component authoring and actual hydration are essential; a small, locally understandable implementation is preferred; esbuild stays. Existing consumers belong to the user and may be migrated deliberately.

**Recommended specification:** the execution, ownership, scheduling, serialization, and form policies below are the proposed starting contract. A local agent may improve implementation choices without asking again about compatibility. Record any semantic change in the decision log before changing fixtures and consumers.

Repository references use the previously reviewed snapshot `95d06aea3bc5b70b7f7df3253649673fef4da35a`. Recheck the actual checkout before editing; filenames and interfaces may have changed. The supplied code sketches, if present elsewhere in this handoff, are API proposals rather than working libraries.

## 2. The execution model

Use synchronous function components that execute **once per instance**. Server rendering creates one server instance. Browser hydration creates a corresponding browser instance from the same initial data. A later reactive update does not rerun that component function. A later explicitly created instance does execute its function.

Components return an internal JSX description. The renderer interprets that description for either SSR or DOM adoption. Intrinsic elements, nested function components, text, arrays of static children, and fragments form the initial tree. Ordinary props are initial values. Pass a reactive object explicitly when a prop must remain reactive.

Recommended authoring semantics:

| Expression | Meaning |
| --- | --- |
| `{count}` where `count` is a signal | Live text binding. |
| `{count.value}` | Snapshot read during setup. |
| `{label}` where `label` is a computed | Live derived text binding. |
| `hidden={closed}` where `closed` is reactive | Live boolean property/attribute binding. |
| An ordinary string prop | Initial value for this instance. |
| A function prop | An ordinary callback, never automatically a reactive getter. |

With a conventional JSX transform, `.value` is evaluated before the JSX runtime receives the result. Do not silently reinterpret it as a tracked template expression. Derived live expressions must be wrapped in `computed`; arbitrary child functions are rejected unless a later explicit structural API requires them. TypeScript supports selecting the automatic JSX runtime through `jsxImportSource`. [TypeScript JSX import source](https://www.typescriptlang.org/tsconfig/jsxImportSource.html)

Recommend `signal`, `computed`, `batch`, an explicit component scope, refs, and root lifecycle functions as the small public surface. Do not introduce hook ordering, dependency arrays, automatic whole-component rerenders, or a Preact compatibility wrapper. A linter diagnostic for `.value` in JSX may help migrations, but normal JavaScript snapshot reads remain valid.

Recommended package location is `packages/zudo-react`. Keep the renderer usable independently of zfb and zudo-wind; zfb owns its embedding and distribution. Proposed exports are `zudo-react`, `zudo-react/jsx-runtime`, `zudo-react/jsx-dev-runtime`, `zudo-react/server`, and `zudo-react/client`. Keep browser APIs out of server-imported modules. This layout is a recommendation, not an existing package.

## 3. What the first version supports

The first version should support:

- Synchronous function components, static composition, text, static arrays, and bounded fragments.
- HTML and a documented SVG subset, with correct namespaces.
- Reactive scalar text and explicitly supported attributes/properties.
- Native event handlers and refs.
- Scoped browser activation and deterministic cleanup.
- SSR, DOM adoption, and a separate client-only mount operation.
- Explicit text-input and checkbox model bindings after their ownership rules pass browser validation.

A signal used as a child may resolve to a scalar text value or an empty value; a signal containing JSX or a changing array is outside this first version. Use a static tab panel with reactive `hidden` and ARIA attributes for the initial tab example. Define whitespace, `null`, `undefined`, and boolean child normalization once and share it across renderers.

The v1 `hidden` binding accepts booleans and reflects DOM state; it does not secretly inject CSS or implement `hidden="until-found"`. Author-level display rules can override the browser's default hidden styling. The tabs specimen therefore includes an explicit authored rule, `.zudo-tab-panel[hidden] { display: none !important; }`, and a case combining that class with `flex` or `grid`. Assert actual computed display when toggling. This rule belongs to the consumer whether it uses zudo-wind or ordinary CSS; the runtime has no CSS-engine dependency. Do not extend it to `until-found` without defining that distinct behavior. [HTML hidden behavior and CSS interaction](https://html.spec.whatwg.org/multipage/interaction.html#the-hidden-attribute)

Initially exclude async components, Suspense, streaming, portals, class components, arbitrary DOM node children, context emulation, generic raw HTML insertion, concurrent rendering, and hydration of framework VNodes. Async event handlers remain allowed; asynchronous application work does not require asynchronous component rendering.

The consumer audit determines whether an excluded capability needs a narrow replacement. A counter demo does not establish readiness to migrate every existing interface.

## 4. Reactive state and scheduling

Recommended signal behavior: reads return the current value immediately; writes use `Object.is` to determine change; object and array mutation requires assignment of a replacement value. State reads are synchronous even when DOM writes are deferred. Readonly signals and writable signals have distinct types.

Computed values are pure derivations. Track dependencies while evaluating them, replace stale dependency edges on reevaluation, detect cycles, and disallow writes during computed evaluation. Reads after an upstream write must observe the new derived value, including inside a batch. DOM subscribers should receive only settled values, avoiding intermediate output from diamond-shaped dependency graphs.

Coalesce DOM binding work in a microtask. `batch(fn)` runs `fn` synchronously and suppresses DOM work until the outermost batch exits and the scheduled flush runs; it does not delay state reads. Deduplicate a binding in the queue and skip disposed bindings. Writes during a flush schedule further work under a documented bounded policy; report loops instead of hanging. Provide an awaitable flush helper for behavioral tests.

Avoid a general public `effect()` in the first version. Browser effects can start through explicit activation. If a real consumer requires reactive side effects, introduce a scope-owned subscription API with cleanup-before-rerun, cleanup-on-disposal, error handling, and scheduling specified together.

An unobserved computed must not retain upstream subscriptions forever. Either unsubscribe when its final observer disappears or use an equivalent ownership design whose teardown is testable. Server rendering must release its complete graph after producing output. Shared browser state needs an explicit application owner; module-level mutable state must never become implicit shared SSR state.

## 5. Component scopes and DOM ownership

Recommend passing an explicit scope as the second component argument. This avoids global lifecycle registration and makes ownership visible. A scope owns child scopes, DOM-binding subscriptions, event listeners, refs, and application cleanup callbacks. Its parent controls its lifetime.

`scope.onActivate(callback)` records browser work during setup and runs only after successful hydration or mount. The callback receives access to assigned refs and may return a synchronous cleanup function. It never runs during SSR or failed hydration. Reject promise-valued activation cleanup; start asynchronous work inside the callback and register its cancellation synchronously.

Assign all adopted refs before activation callbacks run. Use a fixed activation order, preferably children before parents. On disposal, mark the root inactive, block future scheduled work, dispose child scopes, run cleanup in reverse registration order while nodes remain available, and clear refs. Disposal must be idempotent. Continue other cleanup if one callback throws, then report the errors with component/root context.

Keep **resource disposal** distinct from **DOM removal**. Disposing a root releases subscriptions and handlers; a separate unmount operation may then remove its owned DOM. This lets the zfb router control body replacement without the runtime independently clearing a persisted region.

Native event callbacks can be async. Surface rejected promises through the root error reporter. A scope should expose an abort signal for fetches and similar work. An operation that finishes after disposal must not reactivate bindings or update detached DOM.

## 6. Server serialization and data transport

Use one normalized description with separate server and browser interpreters. JSX creation should describe nodes; it should not eagerly create DOM or serialize nested components outside the renderer's scope handling.

SSR must escape ordinary text and attribute values; apply documented rules for boolean attributes, omitted values, void elements, `data-*`, ARIA strings, SVG attributes, and namespaces. Serialize event handlers nowhere. Distinguish attribute serialization from property assignment: a generic `setAttribute` loop cannot implement all form and boolean behavior.

Reject invalid/unsupported child objects and suspicious attribute names with useful locations. A plain object must never silently become `[object Object]`. Keep user strings as text rather than markup. Generic inline `script`, `style`, or trusted HTML insertion needs a separately designed escape hatch; zfb's document assembly may own such output outside the runtime's generic text serializer.

Hydrated islands receive JSON-compatible **initial props**, not closures, DOM nodes, signals, class instances, or arbitrary component descriptions. Reject cycles, non-finite numbers, `BigInt`, and unsupported values rather than silently dropping them. Construct signals and handler closures from the imported component module on the browser side. A callback within one runtime tree is valid; a callback crossing the serialized island boundary is not.

Record a transport schema version, runtime protocol version, component/export identity, and build identity with each island. Verify them before activation so stale HTML and a different client bundle cannot accidentally hydrate together.

If embedding JSON in a data script, escape `<` as `\u003c` in JSON text so payloads cannot end the script element; use a dedicated serializer and test closing-tag payloads. Attribute-embedded JSON requires a different HTML-attribute escaping step. Never interpolate JSON directly into executable JavaScript. HTML parsing rules still apply inside data script elements. [HTML script content restrictions](https://html.spec.whatwg.org/multipage/scripting.html#restrictions-for-contents-of-script-elements)

Each render receives an isolated context containing its marker counters, root identity, scope, and diagnostic state. A renderer function installed on the V8 global can remain stateless; per-page mutable render state must not live there.

## 7. Hydration protocol and actual adoption

Hydration must attach to existing DOM objects. Replacing `innerHTML` and attaching events is not an acceptable implementation of this promise. A separate mount operation is appropriate for explicitly client-only roots.

Use deterministic root and region boundaries, with namespaced comment markers as the initial implementation recommendation. Identifiers must derive from a render-local traversal under a root identity supplied by zfb; never use time, randomness, process-global counters, or unrelated page order.

Markers must disambiguate adjacent text bindings, static text next to reactive text, fragments, empty components, empty arrays, and reactive values initially empty. SSR may need separators even when a value has no visible text. Browser parsing can merge adjacent text; an empty slot may need a Text node inserted between its existing boundary markers during commit. This is allowed while retaining existing elements and nonempty owned text nodes.

Do not assume every HTML parsing context permits the same marker strategy. Explicitly handle or reject unsupported positions in raw-text elements, textarea content, select/option structures, and tables. The HTML/SVG namespace transition at `foreignObject` also needs a fixture before claiming support. Author valid table structures rather than relying on hydration to repair them.

Recommended algorithm:

1. Validate root, protocol/build identity, serialized props, and current lifecycle eligibility.
2. Execute setup once in a provisional scope to obtain the expected description. Setup must have no external browser effects.
3. Traverse the existing DOM against that description, recording adoption and binding operations. Verify marker balance, structure, tag names, namespaces, static content, and expected initial reactive content without changing the DOM.
4. Complete form-state reconciliation planning with explicit exceptions for user-editable live properties.
5. Commit node bindings and refs, reconcile model state, register listeners/subscribers, then activate scopes. Recheck cancellation before commit.

Do not execute components a second time to perform commit. Preserve the provisional instances and their closures. Static documents that will never hydrate can omit markers through an explicit server-render mode.

## 8. Mismatch and error policy

The recommended first-version policy is **fail closed for the affected island**: a structural, initial-content, or version mismatch produces an actionable diagnostic and leaves server HTML and user state in place. Dispose the provisional graph. Do not silently remount, overwrite inputs, or continue with a partially adopted tree. Other independent roots may still activate.

Diagnostics should include island identity, component stack, expected and actual structure, approximate DOM path, and protocol/build identifiers. Avoid including complete prop payloads in diagnostics by default.

Preflight is side-effect-free only if component setup follows the contract. Browser APIs, observers, storage reads that alter initial output, and external subscriptions belong in activation. Tooling should detect common violations where practical. If user activation later throws, undo registered runtime resources and report an activation failure; arbitrary effects already executed by user callbacks cannot be perfectly rolled back. Keep this distinction honest.

If a future product decision adds a remount fallback, make it explicit and separately tested. It is not required for the first implementation.

## 9. Form ownership and Japanese input

Uncontrolled form fields should be the default. Their initial/default value establishes SSR output, while the browser owns the live value thereafter. Hydration reads existing nodes without reassigning `.value`, `.checked`, selection, or focus. Do not compare a dirty live property against the server value as if it were a structural mismatch. The HTML standard distinguishes current input value from default/attribute state. [HTML input value behavior](https://html.spec.whatwg.org/multipage/input.html#dom-input-value)

For controlled fields, recommend an **explicit writable model binding** rather than inferring two-way behavior from every reactive attribute. Proposed syntax is `modelValue={name}` for text input/textarea and `modelChecked={enabled}` for checkboxes. `defaultValue` and `defaultChecked` denote uncontrolled initial values; map them to the appropriate SSR representation. These API names are provisional. Reserved runtime props must never appear as HTML attributes. Reject conflicting model/default/value declarations, and reject reactive `value` or `checked` props that would bypass the explicit model policy. Initially support text input, textarea, and checkbox separately; defer select, radio groups, file inputs, and contenteditable until required and specified.

Recommended hydration policy for a model binding: the existing DOM wins at activation. Read the live value/checked state into the writable model before registering subscriptions or performing initial DOM writes, then activate dependent bindings together. This preserves typing, autofill, and checkbox changes before deferred hydration. Readonly computed values cannot serve as models. If several controls bind one model and disagree at hydration, diagnose the conflict rather than allowing traversal order to pick a winner.

After activation, DOM edits update the model and programmatic model writes update the control. Skip assignments when the actual property already equals the desired value, preserving selection where possible. Recommended reset behavior follows the browser's default reset: after an uncancelled form reset completes, read reset DOM values back into the writable models in one batch. A cancelled reset changes neither. Freeze this behavior and any validation conversions in each supported control adapter; do not apply text conversion rules to every input type.

Track composition and avoid programmatic value replacement while an IME composition is active. Buffer external model writes for the control; on composition completion, use the final DOM edit as authoritative under the recommended policy. Native `compositionstart`, `compositionend`, and `InputEvent.isComposing` describe the composition lifecycle. [UI Events composition/input specification](https://w3c.github.io/uievents/#dom-inputevent-iscomposing)

Because hydration may be deferred, the shared boot path should track composition before scheduling islands. If the focused control's composition status is unknown at activation, preserve its value and postpone value writes until composition completion or blur establishes a safe boundary. Browser validation must include actual Japanese IME interaction; synthetic events alone cannot prove the full behavior.

## 10. Extending structure only when needed

The next structural primitives, if required by audited consumers, should be explicit conditional regions and keyed lists. Their names may be `Show` and `For`, but naming is not yet fixed.

Specify region boundary ownership, child-scope creation/disposal, duplicate-key errors, initial SSR identity, and identity-preserving list moves before implementation. Moving a keyed item retains its DOM, scope, input state, and subscriptions. Removing it disposes its scope once. Re-adding a removed key creates a new instance. Hiding a static panel does not dispose it.

These primitives would receive explicit deferred child factories; ordinary callback props retain ordinary callback semantics. Add them after the leaf-binding contract works, rather than implementing an implicit general reconciler. If a selected migration requires changing lists immediately, implement that stage before declaring the consumer migrated.

## 11. zfb integration is broader than an adapter

The reviewed adapter module chooses a framework once during configuration. Preact's shim currently calls `hydrate(h(Component, props), element)`. The existing seam is useful but does not already provide independent per-island renderer selection. [Adapter selection](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-render/src/adapters/mod.rs), [Preact adapter](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-render/src/adapters/preact.rs)

Audit framework enums/config validation, JSX import sources in every compilation path, embedded-package aliases and exports, server rendering shims, VNode-sensitive wrappers, MDX output, island wrappers/prop serialization, generated bundles, and unmount glue. Standard production/development JSX entry points must export the functions actually emitted by the configured transforms. zfb also has a SWC transformation path; keep its JSX configuration consistent while leaving esbuild in place. [SWC pipeline](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-render/src/swc_pipeline.rs)

Use a dedicated migration fixture/project with one selected framework initially. Retaining Preact in another fixture during development is acceptable. Do not assume VNodes, context, or nested ownership can pass between the runtimes. Supporting multiple renderers in one page would be a separate zfb orchestration feature with explicit DOM boundaries.

Keep zfb responsible for when islands activate. Return/store a root handle so its existing lifecycle can dispose resources. Cover immediate, idle, visible, media, cancellation during import, removal before activation, client-only mount, and duplicate activation. Persisted islands with unchanged identity/props retain the same DOM and scope. A changed component or props needs an explicit dispose-and-recreate lifecycle path; it must not pretend to update initial props silently. [zfb island runtime](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/packages/zfb/src/runtime.ts)

## 12. Implementation sequence and release evidence

1. **Audit and freeze:** inventory real consumer behavior, decide the first supported element/property table, capture migration examples, and record API/marker/version choices.
2. **Build the semantic core:** typed JSX descriptions, component scopes, signals, computed dependency cleanup, scheduling, and SSR serialization. No browser compatibility claims yet.
3. **Implement adoption:** preflight, markers, refs, leaf bindings, native events, mismatch disposal, and root handles. Prove node identity in a real browser.
4. **Integrate one zfb fixture:** server rendering through V8, current bundling, island scheduling, navigation and persistence. Verify a release build, not just a development import.
5. **Finish forms and required structure:** prehydration edits, IME, cleanup, and any audited list/conditional requirements.
6. **Migrate selected consumers:** rewrite their semantics explicitly, remove stale imports and framework-shaped wrappers, and retain dependency code only for consumers still being migrated.

Required evidence includes node-identity assertions; adjacent and empty text; fragment siblings; HTML escaping and JSON closing-tag payloads; independent SSR renders; reactive diamond graphs; subscriber counts after repeated disposal; errors before and during activation; transport mismatch; cancellation during delayed loading; persistence without double activation; async completion after removal; editable input/selection preservation; checkbox and reset behavior; and supported SVG/table cases.

Also verify repeated copies of one component receive independent root/region identities, nested independently owned island roots are rejected or explicitly excluded from parent ownership, and the server page remains useful with JavaScript disabled. Native links and ordinary form semantics should survive delayed or failed activation.

Use browser tests for browser semantics and focused unit tests for language rules. Capture a manual Japanese IME check as a distinct result. Record actual bundle bytes, activation cost, and update behavior for the chosen fixture without inventing performance targets or claiming a speedup before measurement. Completion means the selected real consumer satisfies its declared behavior under this runtime, with its migration documented and unsupported cases visible.

## 13. Illustrative authoring specimen

This sketch shows the recommended semantics. It is not executable until the runtime exists; export names and JSX event spelling are provisional API choices for R0 to freeze. `onClick` below means a native click listener with no synthetic-event layer. The specimen deliberately uses ordinary classes so it can be verified independently of zudo-wind.

```tsx
import { computed, signal } from "zudo-react";

type CounterProps = {
  initialCount: number;
  initialName: string;
};

export function Counter(props: CounterProps) {
  // Setup runs once on the server and once for this browser instance.
  const count = signal(props.initialCount);
  const name = signal(props.initialName);
  const label = computed(() => `${name.value}: ${count.value}`);

  return (
    <section class="counter">
      <label>
        Name
        <input type="text" modelValue={name} />
      </label>
      <p>{label}</p>
      <button type="button" onClick={() => { count.value += 1; }}>
        Increment
      </button>
      <p>Initial count: {count.value}</p>
    </section>
  );
}
```

`{label}` updates because a computed object reaches the JSX runtime. The final `{count.value}` is intentionally an initial snapshot. The form adapter owns `modelValue`; it must preserve an edit made before hydration and then update the computed label from the adopted value. Client code reconstructs these signals from serialized initial props. Neither the signal objects nor event closures cross the server-to-browser JSON boundary.

Browser observers, subscriptions, and timers belong inside `scope.onActivate(...)` in a component accepting the second scope argument, with synchronous cleanup registration. This specimen does not require them. Do not add lifecycle machinery to a component that owns no external work.
