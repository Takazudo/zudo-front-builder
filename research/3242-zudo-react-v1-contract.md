# zudo-react v1 contract

## Status and authority

Contract version **1**, revision **1.0.1**, frozen by #3247 on 2026-09-28 for epic #3242. This is the implementation oracle, not a claim that a runtime or browser test already passes. Authority, in order: owner direction quoted in the epic; this owned contract; epic and sub-issue bodies; the original handoff; exploration maps. This is the order in `_temp-resource/3242-owned-engines/README.md`. A semantic change requires a recorded revision and updated affected issue locks before implementation. In the later documentation reconciliation (#3309), pages describe observed implementation honestly and record any divergence as a contract defect for the manager; documenting observed behavior does not silently amend this implementation oracle.

The runtime lives inside `@takazudo/zfb`, has no runtime dependency, and works with ordinary authored CSS. There are no hooks, context, portals, class components, streaming, general tree reconciliation, or React/Preact compatibility aliases. The frozen `docs/` host remains on its published 2.x stack. The first migrated consumer is `crates/zfb/templates/basic-blog/`.

Evidence labels below refer to stable sections/symbols, not drifting line numbers:

- **L**: `research/3242-owned-engines-ledger.md`, delegated decisions DD3, DD4, DD8, DD14, DD17, DD20; corrections; client-island and raw-HTML inventories.
- **R**: exploration `explore-render-framework.md`, contracts and call sites; especially `write_entry_module`, `render_md_page_shell`, `render_region_marker`, `createPageRouter`.
- **I**: exploration `explore-islands-client.md`, answers 1–6 and contracts; especially `Island`, `readProps`, `clearMountedForRemount`, `swapBodyElement` and livereload.
- **C**: exploration `consumer-requirements-summary.md` and `census-zudo-doc-requirements.md`, runtime inventory. Public facts used here are names and aggregate counts only.
- **E**: exploration `explore-toolchain-embedding.md`, resolution modes and `copy_ts_src`.
- **G**: exploration `census-docs-coupling-gates.md`, byte assertions, HTML validation and minification; `crates/zfb/src/commands/html_minify.rs`.
- **H**: handoff `docs/03-zudo-react.md`, sections 2–11; `resources/acceptance-cases.json`, R-A01–R-A11. These are proposals and unexecuted acceptance cases, subordinate to L.

All exploration and handoff paths above are relative to `_temp-resource/3242-owned-engines/`. That temporary directory is removed before the root PR merges; the decisions and rationale needed for implementation are recorded here and in L.

## Decision index

Each row freezes one policy; the following sections give its exact details.

| ID | Topic | Decision | Evidence |
| --- | --- | --- | --- |
| ZR01 | API names and export map | Ship the five entry points and exact declarations below, with no DOM reachable from the server graph. | L DD3; R `write_entry_module`; E `copy_ts_src` |
| ZR02 | JSX description | Use immutable-by-contract branded descriptions exposing `type`, `props`, `key`, reusable across renders. | R SDK readers; C reused JSX; #3272 six-argument development transform |
| ZR03 | Prop dialect | Use HTML/SVG spellings, CSS-spelled style keys, string/object styles, and reject React spellings. | L DD20; C 425 `class`, 272 `className`, 42 object/8 string styles |
| ZR04 | Server string `on*` | Serialize lowercase string event attributes with attribute escaping; never serialize function listeners. | C one link `onload`; R emitter |
| ZR05 | Custom elements | Accept hyphenated lowercase tags with scalar string attributes and native listeners, without property inference. | C one custom tag |
| ZR06 | Raw HTML | Reserve `rawHtml` for trusted static/reactive content; opaque hydration and restricted raw-text use. | L DD8 and raw-HTML inventory; C 38 sites, 11 inside islands |
| ZR07 | Return shapes | Render synchronous descriptions, arrays, scalar text and empty values; reject nested promises. | R `ClientRouter` arrays and page router |
| ZR08 | Construction and inspection | Export `h`, `isDescription`, `flattenChildren`; copying descriptions is ordinary object copying. | R `.ts` pages; C 13 literals, 2 child-array calls, 1 clone |
| ZR09 | Reactivity | Use `Object.is`, synchronous reads, lazy computed values and microtask DOM/effect flushing capped at 100 passes. | H section 4; C consumer server-test exposure |
| ZR10 | Scope and plain calls | Export setup-only `getScope()`; plain calls share the current owner and allocate no independent scope. | C 27 `Island()` and one `ClientRouter()` call |
| ZR11 | Effects | Provide `scope.effect`, activated only in browsers after DOM commit, with cleanup before rerun and disposal. | L DD8; C 31 of 52 effects rerun |
| ZR12 | Markers and identity | Keep the wrapper, bind transport/protocol/build/component identity, and use local paired comment regions only inside islands. | L DD14; I scanner versus function-name mismatch |
| ZR13 | Hydration and mismatch | Adopt in preflight/commit phases, fail closed per island, and enforce the named parser-context restrictions below. | H sections 7–8; G |
| ZR14 | Minification tolerance | Compare parsed structure and normalized ordinary text; preserve exact preformatted text and opaque regions. | G comments retained, whitespace collapse, attribute unquoting |
| ZR15 | Forms | Use explicit writable-branded models, rejecting readonly/computed models at compile time and runtime, with DOM winning at hydration; support five adapters, single-select only, with composition tracking. | L DD8/DD17; C 5 selects, 4 radio uses, one composition guard |
| ZR16 | Conditions and lists | Export `Show` and `For` with owned branch/item scopes, keyed moves, and transactional duplicate-key rejection. | L DD8; C 9 of 19 islands need structure; basic-blog theme toggle |
| ZR17 | Island boundary | Require one component child, strict JSON props and scanner identity agreement; reject nesting; retain valid hand-authored wrappers. | I lossy serialization/multi-child ambiguity; L DD14 |
| ZR18 | Lifecycle | Separate disposal from removal; recreate changed component/props in `render` mode and preserve unchanged persisted roots. | I remount after destructive unmount; router persistence |
| ZR19 | Error isolation and handles | Store handles at `Symbol.for("@takazudo/zfb/zudo-react/root-v1")`; isolate and report each root's errors. | I module re-import loses private maps |
| ZR20 | Skip SSR | Apply `data-when` to skip-SSR roots and atomically replace fallback at successful client mount. | C 9 fallback uses; I immediate skip-SSR behavior |
| ZR21 | Temporary seams | Use the exact factory/boundary and emitter seams below, then remove all selectors in #3288. | L DD4; R four hard-coded producers; E three resolution modes |

## Public API and exports

| Entry point | Complete value exports | Type exports |
| --- | --- | --- |
| Core | `Fragment`, `h`, `isDescription`, `flattenChildren`, `signal`, `computed`, `batch`, `flush`, `getScope`, `Show`, `For` | `Key`, `Scalar`, `ReadonlySignal`, `Signal` (required writable brand), `Child`, `Component`, `ElementType`, `Description`, `Cleanup`, `Scope`, `Ref`, `Style`, `Listener`, `IslandIdentity`, `Diagnostic`, `Reporter`, `ShowProps`, `ForProps` |
| JSX runtime | `Fragment`, `jsx`, `jsxs` | `JSX` |
| JSX development runtime | `Fragment`, `jsxDEV` | `JSX` |
| Server | `renderToString`, `islandRoot`, `serializeProps` | `RenderOptions`, `IslandOptions` |
| Client | `hydrate`, `mount`, `parseProps` | `RootOptions`, `RootHandle` |

The following declarations are normative signatures, not implementation code. This is the entire final public export surface of these five entries; test introspection and internal scope factories are not public. Type declarations are exported only where stated. Every source file is `.ts`, relative imports end in `.js`, and no runtime source path segment starts with `__` or `.`. The server and core entries import neither the client entry nor the rest of the SDK. Type-only DOM references are allowed; runtime DOM access is confined to client modules.

### Core: `@takazudo/zfb/zudo-react`

```ts
export type Key = string | number;
export type Scalar = string | number | boolean | null | undefined;
export interface ReadonlySignal<T> {
  readonly value: T;
  readonly $$zudoReactive: "zudo-react.reactive.v1";
}
export interface Signal<T> extends ReadonlySignal<T> {
  value: T;
  readonly $$zudoWritable: "zudo-react.writable.v1";
}
export type Child = Scalar | Description | ReadonlySignal<Scalar> | readonly Child[];
export type Component<P = Record<string, unknown>> = (props: P) => Child;
export type ElementType = string | Component<any> | typeof Fragment;
export interface Description {
  readonly $$zudo: "zudo-react.description.v1";
  readonly type: ElementType;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: Key | null;
}
export type Cleanup = () => void;
export interface Scope {
  readonly abortSignal: AbortSignal;
  onActivate(fn: () => void | Cleanup): void;
  onCleanup(fn: Cleanup): void;
  effect(fn: () => void | Cleanup): void;
}
export interface Ref<T> { current: T | null }
export type Style = string | Readonly<Record<string, string | number | null | undefined>>;
export type Listener<E extends Event = Event> = (event: E) => void | Promise<void>;
export interface IslandIdentity { readonly component: string; readonly build: string }
export interface Diagnostic {
  readonly code: string;
  readonly phase: "setup" | "preflight" | "commit" | "activation" | "update" | "cleanup";
  readonly component: string;
  readonly componentStack: readonly string[];
  readonly path: string;
  readonly expected: string;
  readonly actual: string;
  readonly protocol: string;
  readonly build: string;
}
export type Reporter = (diagnostic: Diagnostic) => void;
export interface ShowProps {
  when: ReadonlySignal<boolean>;
  children: () => Child;
  fallback?: (() => Child) | undefined;
}
export interface ForProps<T> {
  each: ReadonlySignal<readonly T[]>;
  by: (item: T) => Key;
  children: (item: ReadonlySignal<T>, index: ReadonlySignal<number>) => Child;
}
export const Fragment: unique symbol;
export function h(type: ElementType, props: Record<string, unknown> | null,
  ...children: Child[]): Description;
export function isDescription(value: unknown): value is Description;
export function flattenChildren(children: Child): readonly Exclude<Child, readonly Child[] | boolean | null | undefined>[];
export function signal<T>(initial: T): Signal<T>;
export function computed<T>(read: () => T): ReadonlySignal<T>;
export function batch<T>(fn: () => T): T;
export function flush(): Promise<void>;
export function getScope(): Scope;
export function Show(props: ShowProps): Description;
export function For<T>(props: ForProps<T>): Description;
```

`Signal<T>` adds the required string-literal brand `$$zudoWritable: "zudo-react.writable.v1"`. Every `signal()` result carries this property as well as `$$zudoReactive`; `computed()` results do not carry `$$zudoWritable`. `ReadonlySignal<T>` exposes only the shared reactive brand and readonly value. Thus a writable signal remains assignable to a readonly signal, while a value typed as `ReadonlySignal<T>` (including a computed or a readonly view) is not assignable to `Signal<T>`. TypeScript's structural assignability does not distinguish a readonly property from a writable property, so `readonly value` alone cannot enforce the model boundary. The additional required brand supplies that distinction without a new export, setter API, or local-symbol identity. It is a structural API discriminator, not protection against deliberate type assertions or forged objects; callers obtain writable models through `signal()`.

`Fragment` is implemented with a well-known symbol (`Symbol.for("@takazudo/zfb/zudo-react/fragment-v1")`) while its declaration uses `unique symbol` for type discrimination. Factories never run components. `Show` and `For` return inert built-in descriptions; their deferred factories run only under a renderer-owned scope. `Component<any>` in the element-type union is the deliberate erased call boundary, not permission to bypass intrinsic JSX typings.

### JSX entries

`@takazudo/zfb/zudo-react/jsx-runtime` exports exactly:

```ts
export { Fragment } from "./index.js";
export function jsx(type: ElementType, props: Record<string, unknown> | null,
  key?: Key | undefined): Description;
export function jsxs(type: ElementType, props: Record<string, unknown> | null,
  key?: Key | undefined): Description;
export type { JSX } from "./jsx-types.js";
```

`@takazudo/zfb/zudo-react/jsx-dev-runtime` exports exactly:

```ts
export { Fragment } from "./index.js";
export function jsxDEV(type: ElementType, props: Record<string, unknown> | null,
  key: Key | undefined, isStaticChildren: boolean,
  source: { fileName: string; lineNumber: number; columnNumber: number } | undefined,
  self: unknown): Description;
export type { JSX } from "./jsx-types.js";
```

Types referenced in entry signatures come from the core; these entries do not re-export them. The type namespace `JSX` exports `Element = Child`, `ElementType`, `IntrinsicElements`, `IntrinsicAttributes { key?: Key | undefined }`, and `ElementChildrenAttribute { children: unknown }`. Its intrinsic map follows the dialect/control tables below, with no general string prop index except `data-*`, `aria-*`, and the specified custom-element attributes. `Show`/`For` factory children are typed explicitly, not admitted as ordinary intrinsic children.

### Server: `@takazudo/zfb/zudo-react/server`

```ts
export interface RenderOptions { island?: IslandIdentity | undefined }
export interface IslandOptions {
  identity: IslandIdentity;
  when?: "load" | "idle" | "visible" | "media" | undefined;
  media?: string | undefined;
  skipSsr?: boolean | undefined;
  fallback?: Child;
}
export function renderToString(node: Child, options?: RenderOptions): string;
export function islandRoot(child: Description, options: IslandOptions): Description;
export function serializeProps(props: Record<string, unknown>): string;
```

`islandRoot` is the explicit zfb-facing wrapper constructor: it returns an inert built-in description which emits the complete wrapper shown below. It runs neither the component nor serialization until rendered. `renderToString(node, { island: identity })` is the standalone inner-output mode: it emits the same child markers without a wrapper, for a caller that supplies the matching container attributes. Without this option only explicit `islandRoot` subtrees carry runtime markers. `serializeProps` returns JSON text, not HTML-escaped text; the renderer escapes it once as an attribute.

### Client: `@takazudo/zfb/zudo-react/client`

```ts
export interface RootOptions {
  identity: IslandIdentity;
  signal?: AbortSignal | undefined;
  report?: Reporter | undefined;
}
export interface RootHandle {
  readonly protocol: "zudo-react/1";
  readonly identity: IslandIdentity;
  readonly disposed: boolean;
  dispose(): void;
  unmount(): void;
}
export function hydrate(node: Child, container: Element, options: RootOptions): RootHandle | null;
export function mount(node: Child, container: Element, options: RootOptions): RootHandle | null;
export function parseProps(json: string): Record<string, unknown>;
```

`hydrate` and `mount` report expected failures and return `null`; they never return a partly active handle. A failed duplicate call leaves the prior live handle alone. The default reporter calls `console.error`. If a supplied reporter throws, the runtime catches it, reports to the console and continues cleanup and other roots. `parseProps` throws a named transport error on invalid input. Shared types (`Child`, `IslandIdentity`, `Reporter`) are imported from the core; only the declarations above are re-exported from client/server.

### Export maps

These are the exact five additions to `packages/zfb/package.json`. The source `exports` entries are:

```json
{
  "./zudo-react": { "types": "./src/zudo-react/index.ts", "default": "./src/zudo-react/index.ts" },
  "./zudo-react/jsx-runtime": { "types": "./src/zudo-react/jsx-runtime.ts", "default": "./src/zudo-react/jsx-runtime.ts" },
  "./zudo-react/jsx-dev-runtime": { "types": "./src/zudo-react/jsx-dev-runtime.ts", "default": "./src/zudo-react/jsx-dev-runtime.ts" },
  "./zudo-react/server": { "types": "./src/zudo-react/server.ts", "default": "./src/zudo-react/server.ts" },
  "./zudo-react/client": { "types": "./src/zudo-react/client.ts", "default": "./src/zudo-react/client.ts" }
}
```

The matching published `publishConfig.exports` entries are:

```json
{
  "./zudo-react": { "types": "./dist/zudo-react/index.d.ts", "default": "./dist/zudo-react/index.js" },
  "./zudo-react/jsx-runtime": { "types": "./dist/zudo-react/jsx-runtime.d.ts", "default": "./dist/zudo-react/jsx-runtime.js" },
  "./zudo-react/jsx-dev-runtime": { "types": "./dist/zudo-react/jsx-dev-runtime.d.ts", "default": "./dist/zudo-react/jsx-dev-runtime.js" },
  "./zudo-react/server": { "types": "./dist/zudo-react/server.d.ts", "default": "./dist/zudo-react/server.js" },
  "./zudo-react/client": { "types": "./dist/zudo-react/client.d.ts", "default": "./dist/zudo-react/client.js" }
}
```

There is no default export, new package, `browser` condition, or Node-only import. Consumers set `jsxImportSource: "@takazudo/zfb/zudo-react"`. Source, packed, and embedded resolution must all pass; a TypeScript `paths` shortcut is not evidence of package resolution.

## JSX descriptions and prop dialect

Descriptions carry the enumerable string property `$$zudo` with the literal brand above. Reactive values use their different brand. Neither brand depends on `instanceof` or a local symbol. Factories shallow-copy props, extract `key` (the factory's third argument takes precedence over `props.key`), and never mutate caller objects. Absent keys normalize to `null`; finite numbers and strings are valid keys. `h` uses `props.children` when no variadic children were supplied; otherwise the variadic list replaces it. Rendering never mutates or stores per-render state on descriptions. Copying `{ ...description, props: { ...description.props, ...changes } }` is valid. Unbranded hand-built literals must migrate to `h`.

`flattenChildren` flattens arrays, removes null/undefined/booleans, and keeps strings (including empty strings), finite numbers, descriptions and scalar reactive objects. It does not execute components or read reactive values. Invalid objects, functions, symbols, bigints, promises and non-finite child numbers fail with a value-kind and parent/path diagnostic. A reactive child may resolve only to `Scalar`; changing trees uses ZR16. A `.value` expression is a snapshot. Only passing the reactive object creates a live binding.

A renderer invokes each function component once per instance. Top-level page functions may be async because `createPageRouter` awaits them before calling the renderer; its existing `Response` and complete HTML string passthrough are outside this serializer. Strings passed directly to `renderToString` are escaped text. Nested async components and promise children fail with `ZR_ASYNC_COMPONENT`.

The intrinsic map accepts standard HTML elements with their HTML-spelled attributes. The SVG subset is `svg`, `g`, `path`, `circle`, `ellipse`, `rect`, `line`, `polyline`, `polygon`, `text`, `tspan`, `defs`, `symbol`, `use`, `clipPath`, `mask`, `linearGradient`, `radialGradient`, `stop`, `title`, `desc`, `foreignObject`. SVG spellings include `viewBox`, `preserveAspectRatio`, `gradientUnits`, `gradientTransform`, `markerWidth`, `markerHeight`, `refX`, `refY`, `xlink:href`, `xml:lang`, and hyphenated presentation names such as `stroke-width`, `fill-rule`, `clip-rule`, `stroke-linecap`, `stroke-linejoin`, `stop-color`, `stop-opacity`. Unsupported SVG tags/attributes fail by name. The SVG namespace starts at `svg`; children of `foreignObject` return to HTML, with nested `svg` switching back.

HTML examples are `class`, `for`, `charset`, `datetime`, `tabindex`, `readonly`, `autofocus`, `maxlength`, `http-equiv`, `accept-charset`. `className`, `htmlFor`, `charSet`, `dateTime`, `tabIndex`, `readOnly`, `strokeWidth`, `dangerouslySetInnerHTML` and other React-only spellings are type errors and runtime `ZR_PROP_DIALECT` errors, never emitted unchanged. Both validators must use the same finite attribute table; dynamic `h` inputs receive the same validation as JSX. Unknown HTML props are rejected. `data-*` accepts strings/numbers/booleans; ARIA values are strings/numbers/booleans with false serialized as `"false"`. Null/undefined omit attributes. Reactive forms of accepted scalar values are allowed except the form-policy exceptions below.

Boolean HTML attributes use presence for true and absence for false, null or undefined. Enumerated attributes, including `contenteditable`, are strings; they do not use boolean omission. `hidden` supports booleans only, not `until-found`; `inert` is boolean. The runtime injects no hiding CSS. The tabs acceptance specimen owns `.zudo-tab-panel[hidden] { display: none !important; }` and verifies it alongside authored flex/grid CSS.

`style` accepts a CSS string or a flat object whose keys are CSS-spelled (`background-color`, `font-size`, `--accent`). Values are strings or finite numbers, with numbers emitted verbatim and no implicit `px`; null/undefined entries are omitted. Entries retain insertion order and serialize as `name:value;`. CamelCase keys, arrays, nested objects and per-entry reactive objects are rejected; the whole style value may be reactive. Browser updates remove formerly owned properties absent from the new object. Arbitrary style strings are trusted CSS, not parsed into a compatibility dialect.

Function-valued event props use `on:click`, `on:input`, etc., with native, case-sensitive event names after the colon; `on:click:capture` selects capture. Only the final `:capture` suffix is special. Both are ordinary `addEventListener` listeners with no synthetic event/delegation layer. The handler receives the native event and native `currentTarget`; refs are `{ current: null }` objects, with no ref factory or callback refs. `onClick` and function-valued `onclick` are rejected with the spelling to use. Lowercase HTML `on*` string attributes (for example `onload`) are the trusted inline-handler form and are static only. They are emitted with normal HTML attribute escaping, which preserves the parsed source string. They are never evaluated by the runtime. Reject competing inline/listener forms for the same event on one element. A listener's rejected promise is reported even after disposal, while disposed bindings cannot update DOM.

A custom element has a valid lowercase hyphenated tag. It accepts string-valued attributes (or reactive strings/null/undefined), the reserved runtime props, and native listeners. It receives no arbitrary object-to-property assignment. Invalid attribute names containing whitespace, quotes, angle brackets, equals or control characters fail before serialization. Custom-element constructors can themselves have browser side effects; the runtime cannot roll those back.

## Reactivity and scopes

Signals compare assignments using `Object.is`, including `NaN` and signed zero. Mutating an object in place is not a notification. Computeds are lazy, replace stale dependencies on reevaluation, reject writes during evaluation, diagnose cycles, and return settled values on reads inside or outside a batch. An unobserved computed retains no upstream subscriptions. Rendering on the server reads values once without observing them and disposes its entire scope graph in `finally`, on success and error.

`getScope()` is valid only during synchronous component setup or a `Show`/`For` factory. The renderer establishes and restores the current owner in `finally`. A direct call made during setup reuses that owner; a pure direct call outside rendering is fine, but calling `getScope()` there throws `ZR_NO_SCOPE`. This deliberately replaces the handoff's second component argument: there is one acquisition mechanism and no detached scope. No context survives an `await`. Pure SDK constructors such as `Island()` and `ClientRouter()` create descriptions without acquiring a scope. Module-level mutable state is not implicit request state: browser shared state is allowed under explicit application ownership, while SSR authors must construct request state within rendering.

Scope registration is setup-only. `onCleanup` registers synchronous resource cleanup; `onActivate` registers browser startup that may return a synchronous cleanup; `effect` registers a tracked browser effect. Promise-valued cleanup is a diagnostic. All refs are assigned before any activation callback. Activation runs children before parents, siblings in output order; callbacks within a scope run in registration order. Effects start in the first scheduler effect phase after all activation callbacks finish. A failed activation disposes the root's runtime resources and reports failure; arbitrary external effects already performed by user code are not reversible.

`batch` runs synchronously and restores batch depth even if it throws. The scheduler uses microtasks only. Each pass settles derivations, commits structural and DOM bindings, then runs effects; a subscriber runs at most once in a phase of that pass. Effects see the committed DOM. Writes during a pass enqueue a subsequent pass; no intermediate diamond value reaches DOM. Before an effect reruns its previous cleanup runs untracked; final cleanup runs at scope disposal. A throwing effect is reported, its old dependencies are detached, and other effects continue; a later invalidation of dependencies read in the failed run may retry it.

One drain is limited to **100 passes**, including the initial pass. If pass 101 would be needed, drop the outstanding jobs of that drain, report `ZR_FLUSH_LIMIT` for each affected root with subscriber identifiers in `actual`, and reject outstanding `flush()` waiters with that code. The automatically scheduled drain consumes its own rejection after reporting; it causes no unhandled promise rejection. A subsequent external write starts a fresh drain. Disposed jobs never run; unrelated settled roots remain usable. `flush()` resolves only after the current DOM/effect queues quiesce, including work enqueued by the drain, and resolves immediately if idle. It does not await arbitrary application promises.

Disposal first marks the scope inactive and aborts its signal, blocks queued jobs, disposes child scopes, then runs its cleanup callbacks in reverse registration order while nodes still exist, and finally clears refs. It continues after errors and reports all failures. Aborting already disposed work cannot reactivate the scope.

## Server rendering, markers, and identity

Static rendering emits no runtime comments or attributes outside explicit island boundaries. It emits no doctype, always emits a literal lowercase `</head>` for a head element, emits paired `template` tags, and preserves prop insertion order with double-quoted attributes. Text escapes `&`, `<`, `>`; attributes additionally escape `"` as `&quot;`. Void HTML tags (`area`, `base`, `br`, `col`, `embed`, `hr`, `img`, `input`, `link`, `meta`, `param`, `source`, `track`, `wbr`) have no end tag and reject children/rawHtml. SVG non-void tags are paired.

The render-region sentinel is separate from this protocol and stays byte-exact before extraction/minification:

```html
<template data-zfb-render-region="start" data-zfb-region-id="ID"></template>
<template data-zfb-render-region="end" data-zfb-region-id="ID"></template>
```

No fragment separator or whitespace is inserted into static output. Attribute order above is mandatory for SDK-created sentinels. Ordinary component/array boundaries emit nothing in static mode.

### Wrapper and identity

`islandRoot` emits the following attributes in this order, before its children:

| Attribute | Exact value/rule |
| --- | --- |
| `data-zfb-island` OR `data-zfb-island-skip-ssr` | Static scanner marker name, exactly one of these attributes |
| `data-when` | `load` (default), `idle`, `visible`, or `media` |
| `data-media` | Required nonempty query only for `media`; omitted otherwise |
| `data-zfb-transport` | `json/1` |
| `data-zfb-protocol` | `zudo-react/1` |
| `data-zfb-build` | Nonempty opaque build identity supplied by zfb |
| `data-props` | Always present, including `{}`; strict JSON with one attribute-escaping pass |

The wrapper is a `div`. Identity is scoped by the wrapper element itself plus component/build; no global root counter or globally unique element ID is required. Separate instances of the same component intentionally reuse local region numbers. Diagnostics distinguish them with the root's DOM path. Both caller-provided `RootOptions.identity` values must agree with the wrapper before setup; the fixed transport/protocol strings must also match.

The scanner's static `marker_name` is authoritative. For v1 the server's `type.displayName ?? type.name` must equal it exactly: a conflicting display name is an error, not an alternate alias. #3284 preserves function names in the server bundle using esbuild `keep-names`, validates rendered names against the scanned manifest, and upgrades a missing/ambiguous owned-runtime name to a build error. Client glue uses the same static scanner name. Anonymous/non-function/unregistered children are rejected. This deliberately requires renaming/removing conflicting `displayName` assignments (21 measured downstream assignments); it avoids adding a second component registry API. The renderer-only harness supplies an explicit identity without a zfb manifest, but still checks the child's name when using `islandRoot`.

The build identity is one deterministic digest chosen before either bundle is emitted from the normalized source/config/runtime-protocol inputs that affect the server or client graph. It is not derived from the final bundle containing that digest (no hash cycle). The generated server entry exposes it as `globalThis.__zfb.zudoReactBuild` before page evaluation; the SDK boundary reads that build metadata and passes it to `islandRoot`, while the runtime itself never imports SDK state. The same token reaches SSR and shared client glue; changed inputs in dev produce a new token. #3284 owns plumbing and deterministic-input coverage; #3291 verifies stale HTML rejection.

### Marker grammar

The exact comment bodies use ASCII; `N` is a base-10 nonnegative integer without leading zeros (except `0`):

```text
open  := <!--zr:1:N:K[:P]-->
close := <!--/zr:1:N-->
K     := t | f | c | s | l | i | h
P(s)  := 1 | 0
P(i)  := nHEX | sHEX
```

`t` is a scalar reactive text slot; `f` an explicit Fragment or bare array returned by a component; `c` a component instance (including an empty return); `s` a Show region with initial true/false bit; `l` a For list; `i` one keyed item; `h` the content of an ordinary element with `rawHtml`. Only `s` and `i` have `P`. For a key, use `n` plus lowercase hex of UTF-8 `String(number)` or `s` plus lowercase hex of UTF-8 string; numeric `-0` normalizes to `0`. This avoids `--`, comment termination and collision between number/string keys. Empty string keys encode as `s`. The key hex is metadata, not an HTML-escaped attribute.

Allocate numbers at each opening marker, in depth-first preorder starting at 0 for each island root; closes repeat their opener's number. Render-local state is never cached on descriptions or shared between requests. Ordinary intrinsic children arrays are traversal lists, not extra `f` regions; explicit Fragment and a component's returned array do create `f`. Every function component produces `c`; the `islandRoot`, `Show`, `For`, and `Fragment` built-ins produce their own wrapper/region semantics with no extra `c`. Region markers always enclose complete sibling ranges; their nesting must be balanced and exact. Post-activation new regions take fresh monotonically increasing local numbers; disposed numbers are not reused.

### Complete output examples

These six complete wrappers are independent renders. In each case a registered function `Demo` with no props returns the described value, build identity is `b1`, and `when` is omitted. The root component gets region 0. Only these wrappers contain markers; a surrounding document remains marker-free. Output lines have no implicit whitespace.

Adjacent dynamic text: `Demo` returns `h("p", null, signal("A"), signal("B"))`.

```html
<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><p><!--zr:1:1:t-->A<!--/zr:1:1--><!--zr:1:2:t-->B<!--/zr:1:2--></p><!--/zr:1:0--></div>
```

An empty dynamic value: `Demo` returns `h("p", null, signal(null))`.

```html
<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><p><!--zr:1:1:t--><!--/zr:1:1--></p><!--/zr:1:0--></div>
```

A fragment: `Demo` returns `h(Fragment, null, h("b", null, "A"), h("i", null, "B"))`.

```html
<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><!--zr:1:1:f--><b>A</b><i>B</i><!--/zr:1:1--><!--/zr:1:0--></div>
```

A conditional region: `Demo` returns `Show({ when: signal(true), children: () => h("b", null, "Yes") })`; its false branch with no fallback is the same `s:0` pair with no content.

```html
<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><!--zr:1:1:s:1--><b>Yes</b><!--/zr:1:1--><!--/zr:1:0--></div>
```

A keyed list: `Demo` returns a `ul` containing `For` over `[{id:"a",label:"A"},{id:"b",label:"B"}]`, keyed by `id`, whose factory returns `h("li", null, item.value.label)` (a snapshot for this example).

```html
<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><ul><!--zr:1:1:l--><!--zr:1:2:i:s61--><li>A</li><!--/zr:1:2--><!--zr:1:3:i:s62--><li>B</li><!--/zr:1:3--><!--/zr:1:1--></ul><!--/zr:1:0--></div>
```

Raw HTML: `Demo` returns `h("section", { rawHtml: "<em>trusted</em>" })`.

```html
<div data-zfb-island="Demo" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><section><!--zr:1:1:h--><em>trusted</em><!--/zr:1:1--></section><!--/zr:1:0--></div>
```

## Trusted raw HTML and parser contexts

`rawHtml` accepts a string or readonly reactive string. It is mutually exclusive with `children` (including an explicitly supplied empty children prop) and is never an attribute. Ordinary HTML containers receive an `h` region in island mode, even for a static/empty payload. Hydration verifies its boundary and treats everything inside as opaque: it does not compare payload bytes or adopt descendants as runtime bindings. A changed reactive string replaces only that region's contents; equal strings do nothing. Retain the two existing boundary comments on updates. The caller owns trust, valid HTML in that parent context, and cleanup for any external resources inside it. Payloads may not contain `zr:1:` or `/zr:1:` protocol comments or nested island wrappers; reject these reserved boundary shapes. Scripts inserted by client rawHtml are inert; there is no script execution facility.

`script` and `style` permit only static string `rawHtml`, no children, reactive payloads or internal comments. Reject a case-insensitive `</script` substring in a script payload and `</style` in a style payload; do not attempt to escape arbitrary JavaScript/CSS. JSON authors can encode `<` as `\u003c` before supplying trusted script data. Island props use attributes, never executable scripts. Script/style elements themselves can be adopted; payload content is opaque. Ordinary title text is escaped as RCDATA with no marker inside it.

| Parent/context | Supported v1 behavior |
| --- | --- |
| Ordinary HTML flow/phrasing containers | All valid intrinsic children and marker regions; normal HTML nesting must remain valid. |
| `script`, `style` | Static rawHtml only as above; no structural/scalar markers inside. |
| `title` | Static scalar text only; reject reactive children, components and regions. |
| `textarea` | Form adapter owns content; no children, rawHtml or internal markers. |
| `select`, `optgroup`, `option` | Static intrinsic option/optgroup structure and scalar option labels; no components, Fragment, Show, For, rawHtml or reactive text inside. Selection is owned by the form adapter. |
| `table`, `thead`, `tbody`, `tfoot`, `tr`, `colgroup` | Explicit legal intrinsic table children; no scalar text except ignorable whitespace, components or regions at these structural levels. Require explicit `tbody`; reject parser repair such as direct `table > tr`. |
| `td`, `th`, `caption` | Ordinary valid content, including regions, resumes. |
| SVG subset and `foreignObject` | Regions allowed where SVG permits those children; namespace is checked. HTML resumes inside `foreignObject`. RawHtml in SVG is rejected in v1. |
| `template` in an island | Rejected in v1; static page templates and render-region sentinels remain supported. |
| `noscript`, `xmp`, `iframe`, `noembed`, `noframes`, `plaintext` content | No hydrated content or rawHtml; reject in island mode by name. Static empty/ordinary supported element shells do not promise hydration of parser-sensitive content. |

Restrictions apply after component expansion as well as to authored syntax. The wrapper `div` itself must appear in a valid parent (never directly inside a `p`, `table`, `select`, or SVG). Unsupported shapes fail server rendering with element/island/path context. They never rely on browser foster parenting or silently disable markers.

## Hydration, mismatch, and minification

Hydration consumes the child description inside the supplied wrapper, not the wrapper description returned by `islandRoot`. `mount` follows the same child API. The wrapper's children are the runtime's owned DOM; the wrapper itself belongs to zfb.

1. Validate connected container, absent live handle, cancellation, wrapper identity, props transport, and forbidden nested roots. Create one provisional scope tree and run setup once.
2. Preflight without DOM mutation, refs, event listeners, activation or DOM subscriptions: verify the parsed structure, namespaces, region sequence, expected text and initial reactive values. Validate identity and form-control shape attributes explicitly; ordinary attributes follow the structural tolerance policy below. Record node/region adoption operations. Validate every model conflict and read all live form states without writing any model yet.
3. Recheck cancellation/connection immediately before commit. Attach recorded bindings to existing nodes, creating only missing empty Text slots; never recreate existing elements or nonempty text. Assign all refs, reconcile live form states into models in one batch, then subscribe/register listeners. Run activation and enqueue effects/derived updates. Components are not re-executed.

Form-derived text initially matches the original server model during preflight; it reflects reconciled live values in the ensuing flush. Dirty live values, checked state, selection and focus are not preflight mismatches. RawHtml contents are opaque; ordinary inline event attributes are retained without evaluating or rewriting them.

`ZR_HYDRATION_MISMATCH`, `ZR_IDENTITY`, `ZR_PROPS`, `ZR_UNSUPPORTED_POSITION`, `ZR_DUPLICATE_ROOT`, and `ZR_CANCELLED` distinguish failure causes. Every diagnostic has the exact fields in `Diagnostic`; `path` begins at the document path of the wrapper, `expected`/`actual` hold short structure/value-kind summaries, and protocol/build fields describe the attempted client identity. An identity mismatch includes the observed server protocol/build in `actual`. Never include complete props, raw HTML, input values or secrets by default. Cancellation reports `ZR_CANCELLED`, not mismatch. Preflight failure disposes the provisional graph, leaves server DOM and user state unchanged and returns `null`. Runtime-controlled commit failures unwind registered resources and reversible mutations; activation failure leaves committed DOM inert and reports what failed. Neither failure silently calls `mount`.

For `minifyHtml`, compare DOM, not HTML source: attribute order, quote choice, entity spelling and void-slash spelling are irrelevant after parsing. Ordinary attribute differences are deliberately not mismatch inputs: the current minifier also trims/collapses some token values, lowercases some enumerations and removes redundant defaults. Preserve those parsed attributes during initial adoption. Explicit exceptions are wrapper identity/transport and form-control shape (`type`, `multiple`, radio `name`/`value`, option `value` and form association), which are validated semantically by their adapters. Compare an absent text input type as `text`; option values are explicit strings. Unknown extra event/attribute content is not certified by hydration. A subsequent reactive write applies the owned prop, even if the initially adopted attribute differed. This structure-based attribute policy is independent of the minifier's evolving attribute table and intentionally does not detect ordinary static-attribute tampering. In ordinary text, normalize CRLF/CR to LF, collapse each maximal run of ASCII HTML whitespace (`U+0009`, `U+000A`, `U+000C`, `U+000D`, `U+0020`) to one space, then trim leading/trailing ASCII spaces **for comparison only**. Ignore whitespace-only text nodes outside `t` regions. Coalesce expected adjacent static text into a single text run before matching browser text nodes. Do not trim/collapse NBSP or any other Unicode whitespace. Do not mutate an adopted node merely because its spelling differed under normalization.

Inside `pre`, and its descendants, compare decoded text exactly after HTML newline normalization; model-owned textarea content and opaque rawHtml have their separate rules. A literal first LF in a `pre` payload is emitted with an extra LF so the HTML parser's initial-LF removal preserves the authored value. The same protection applies to textarea default/model serialization. A wholly whitespace dynamic `t` value may parse as no text; commit may insert its empty Text slot, preserving all existing nodes. Future live writes use the actual signal string, not the comparison normalization. This is deliberately whitespace-tolerant, not a byte-integrity check: whitespace-only differences cannot diagnose mismatch. Non-whitespace text, tags, namespaces, identity/control metadata and marker identities remain strict.

## Forms

Reserved props are `defaultValue`, `defaultChecked`, `modelValue`, `modelChecked` and never reach HTML under those names. Models require `Signal<T>` with both `$$zudoReactive` and the required `$$zudoWritable: "zudo-react.writable.v1"` brand. A `ReadonlySignal<T>` or `computed()` result is a type error for every model prop. Server and client validation reject model objects without the writable brand, including computed values and dynamic `h` inputs that bypass JSX typing, before any model write; the shared reactive brand alone does not establish writability. A readonly type annotation does not remove a real signal's runtime brand: the type check rejects that readonly view, while runtime validation sees the original writable object. Controls are uncontrolled unless a model is explicitly present. Static `value`/`checked` are allowed as initial defaults; reactive `value`/`checked` are rejected. Supplying both a model and its default/initial value is an error. Supplying both `value` and `defaultValue`, or both `checked` and `defaultChecked`, is also an error. The radio option's static `value` is the deliberate exception: it identifies that radio, so it is required with `modelValue`.

| Control | Binding and type | SSR/default representation | Live event and conversion |
| --- | --- | --- | --- |
| `input` type text/search/email/url/tel/password (text when omitted) | `modelValue: Signal<string>`; `defaultValue: string` | Escaped `value` attribute | `input`; read/write `.value` as a string |
| `textarea` | `modelValue: Signal<string>`; `defaultValue: string` | Escaped text content, initial-LF protection | `input`; read/write `.value` as a string |
| `input type="checkbox"` | `modelChecked: Signal<boolean>`; `defaultChecked: boolean` | Boolean `checked` attribute | `change`; read/write `.checked` |
| single `select` | `modelValue: Signal<string>`; `defaultValue: string` | `selected` on exactly the matching static option | `change`; read/write `.value` after options exist |
| `input type="radio"` group | Shared `modelValue: Signal<string \| null>`; each radio has static `value`, same nonempty `name`, same form/root | `checked` only on the matching option; null checks none | `change` on the newly checked radio; group read is selected string or null |

These type examples are normative (the JSX examples use the owned `jsxImportSource`):

```tsx
const name = signal("Ada");
const label = computed(() => name.value.toUpperCase());
const view: ReadonlySignal<string> = name;
const writable: Signal<string> = name;
const readable: ReadonlySignal<string> = writable;
<input modelValue={name} />;
<textarea modelValue={writable} />;
<p>{label}</p>;
// @ts-expect-error computed values lack the required writable brand
const invalid: Signal<string> = label;
// @ts-expect-error a readonly view does not expose the writable brand
<input modelValue={view} />;
// @ts-expect-error computed values cannot be models
<textarea modelValue={label} />;
// @ts-expect-error boolean computed values cannot be checkbox models either
<input type="checkbox" modelChecked={computed(() => true)} />;
```

All select/radio option values must be unique strings. A programmatic single-select model/default value without a matching option is `ZR_MODEL_VALUE`, with no partial update; hydration can accept browser `.value === ""` with `selectedIndex === -1` as the no-selection state and records `""` without forcing a selection. An empty-valued option remains distinguishable through `selectedIndex`. A radio model with a non-null value absent from the group is likewise rejected; no checked radio maps to null. Group membership is all radios sharing a model within one root, with identical native name/form association; the runtime rejects same-name radios owned by different models/roots. Uncontrolled options may have static `selected` only when no select-level value/default/model is supplied. Uncontrolled radios use native `checked`/`defaultChecked` individually. The model does not own disabled/required validity rules.

**Multiple selection is unsupported in v1:** reject `select[multiple]` in island mode and any model/defaultValue on a multiple select, including array models. Static non-island HTML may render an uncontrolled multiple select with static selected options. File/contenteditable models, numeric/date/range input models, option text reactivity, dynamically changing option structures and changing a control's type/multiple/name while mounted are unsupported. Native uncontrolled versions remain available where they do not violate parser restrictions. Unsupported model placement is both a type error where statically known and a runtime diagnostic.

At activation, all live control states are read and validated before any write; DOM wins in one root-wide batch. Multiple non-radio controls bound to one signal must agree or the root fails preflight. Radio groups reconcile as a group, never by traversal order. `mount` instead initializes newly created controls from the models/defaults before activation; fallback controls are discarded and never adopted. After activation each adapter skips equal property assignments to preserve caret/selection. Native reset is read after its default action in a queued microtask, only if the reset event was not cancelled; all affected models update in one batch. Reset defaults remain the initial SSR/mount defaults, not subsequent model writes.

Composition uses `compositionstart`, `compositionend`, `input.isComposing`, and blur. While active, input can update the model with the live edit but model notifications never write back to that composing control. External writes are buffered for that control and discarded when compositionend/blur reads the final DOM edit as authoritative. The shared boot path **does** install capture listeners before scheduling islands (#3281). It stores per-element state at `Symbol.for("@takazudo/zfb/zudo-react/composition-v1")`, with value `"active"` or `"idle"`; absence means unknown. Installation is idempotent across bundle re-imports via `document[Symbol.for("@takazudo/zfb/zudo-react/composition-tracker-v1")]`, which holds a cleanup function. The runtime forms adapter (#3278) reads this protocol without importing the SDK. A focused control with unknown status postpones writes until compositionend or blur; a nonfocused control is safe. Real Japanese IME remains **deferred human verification** (DD17); synthetic events do not satisfy it.

## Conditional regions and keyed lists

`Show.when` is a readonly boolean signal. The selected factory executes once per branch lifetime in a new child scope; false with no fallback has no child scope. Turning away disposes the old scope once and removes its range; returning creates a fresh instance. A reactive `hidden` attribute only hides existing DOM and disposes nothing.

`For.each` is a readonly array signal; `by` is pure and receives the current plain item. Keys are strings or finite numbers, with `-0` treated as `0`; string `"1"` and number `1` differ. Before touching DOM/data/scope state, validate every next key. Duplicate or unsupported keys report `ZR_DUPLICATE_KEY`/`ZR_KEY_TYPE` and leave the previous list intact. On initial SSR they fail rendering, and on hydration they fail the root. Retained keys keep their range, child scope, refs, event handlers, control state and item/index signal objects. Update those readonly item/index views together, then flush dependent bindings; a changed object with the same key never reruns the factory. Removed keys dispose exactly once; re-added keys create new instances.

Build new branches/items provisionally before replacing old ranges; setup failure disposes the provisional work and leaves the previous region live. A post-commit activation failure follows the root error policy. Moves retain existing nodes: keep the focused item anchored when possible; otherwise restore the same focused element and text selection after moving. During active/unknown focused composition, postpone any reorder that would detach that focused item until compositionend/blur, coalescing to the latest desired list. Removing a composing item is a deliberate unmount and cancels its resources. Regions follow the parser-context table; table-structural and select positions are not supported. Root disposal leaves all region DOM; unmount removes it.

## Island boundary and props transport

`Island` remains an SDK helper, outside these five runtime entries. Its owned path delegates to `islandRoot`, preserving plain-function use. After flattening empty JSX children, require exactly one description whose type is a registered synchronous function component. Host elements, raw strings, multiple components, and nested island boundaries fail at SSR. An island child with nonempty `props.children` is rejected: children cannot silently disappear at the transport boundary. Null/undefined/boolean or recursively empty arrays count as empty and are omitted. Skip-SSR validates identity/props without running its child and renders fallback in static mode, without internal hydration markers. A nested island in fallback also fails.

The top-level props payload must be a plain record. Validation happens recursively before `JSON.stringify`; getters and `toJSON` must not execute. Own enumerable string keys preserve JavaScript enumeration order; nonenumerable properties, accessors and symbol keys are rejected rather than silently lost. Dense arrays permit their intrinsic nonenumerable `length` and own indexed data properties only; reject additional named properties/accessors/symbols. A null-prototype record is accepted and parses as an ordinary data record. `children` is excluded only at the top-level boundary under the empty-child rule; nested properties named children are ordinary JSON data.

| Prop value | Accepted? | Exact disposition |
| --- | --- | --- |
| null, booleans, strings (including `</script`, quotes, ampersands) | Yes | JSON followed by one HTML attribute escaping pass; browser `getAttribute` followed by JSON parse round-trips |
| finite number | Yes | JSON numeric form; negative zero becomes zero, as explicitly defined by transport v1 |
| dense array of accepted values | Yes | Preserve array order |
| plain record / null-prototype record of accepted values | Yes | Preserve own enumerable string keys; no custom serialization |
| repeated reference without a cycle | Yes | Serialize independently by value; object identity does not cross transport |
| undefined (including object fields/array slots), sparse holes | No | `ZR_PROPS_UNDEFINED` with exact property/index path |
| function, symbol, bigint | No | Named unsupported-kind diagnostic with path |
| NaN, positive/negative Infinity | No | `ZR_PROPS_NUMBER` |
| cycle | No | `ZR_PROPS_CYCLE`, current and ancestor paths |
| Date, Map, Set, class instance, DOM node, promise | No | `ZR_PROPS_OBJECT_KIND`; no coercion |
| description, signal, computed | No | `ZR_PROPS_RUNTIME_VALUE`; reconstruct inside client setup |
| accessor, custom `toJSON`, symbol key, nonenumerable own field | No | `ZR_PROPS_PROPERTY`; never invoke accessor/serializer |
| `__proto__`, `prototype`, `constructor` own record keys | No | `ZR_PROPS_KEY`; reject before assignment/merge |

Reject the same forbidden key names recursively during `parseProps`. JSON parsing must produce a top-level record, not null/array/scalar. Missing `data-props`, malformed JSON or missing metadata on an owned root is failure, never `{}` fallback. Hand-authored duplicate JSON keys follow JSON.parse last-value semantics; the resulting record is validated recursively. serializeProps cannot emit duplicates. This avoids adding a second JSON parser to the small runtime. Diagnostics name the island and property path but not the complete value. A bad server payload fails the page render; a bad client payload fails only that island.

Hand-authored persisted wrapper `div` elements **remain supported**, provided they carry every attribute above and inner SSR produced by `renderToString(child, { island: identity })`. No inference of missing metadata or legacy hydration occurs after cutover. Authors can attach the existing `data-zfb-transition-persist` and `data-zfb-transition-persist-props` attributes to that wrapper. Persisted ancestors preserve descendant islands by matching each descendant's boundary identity under the retained DOM; nested island roots themselves remain forbidden. No new persist prop is added to the SDK `Island` surface by this contract.

## Lifecycle and isolation

The final generated manifest mount signature is `(props, element, mode: "hydrate" | "render") => RootHandle | null`; it returns the runtime handle. During the temporary Preact seam #3281 may retain the `void` return and old unmount thunk as a fallback only; #3288 removes that fallback. `hydrate` means adoption, `render` means new client mount. The root key is exactly `Symbol.for("@takazudo/zfb/zudo-react/root-v1")`, stored on the wrapper. The value implements `RootHandle`; a live handle is the guard, never `data-zfb-island-mounted`. The mounted attribute is observational and set only after success. A disposed handle may remain until replaced so a later `unmount()` can remove its owned nodes; it must not remove nodes installed by a newer handle. Its removal ownership is the nodes/ranges captured by that handle, not arbitrary future wrapper children.

`dispose()` is idempotent resource release that leaves DOM; `unmount()` disposes then removes owned children, retaining the wrapper. zfb calls disposal before its own body replacement. Removing a root, cancelling a pending trigger or finding it disconnected prevents future activation. zfb's walk catches mount/dispose/reporter errors per island and continues. Later async application work must honor the scope abort signal; runtime subscriptions and handlers never write after disposal.

Persist with unchanged effective identity/props keeps the same wrapper, DOM, handle and state. Compare decoded component name, root kind (hydrate/skip-SSR), transport, protocol, build, and exact decoded `data-props` JSON string; serialization order is intentionally significant, so semantically equal reordered JSON may recreate. The existing persist-props switch retains old props when its attribute value is present and not `"false"` (absent or `"false"` refreshes), but it never suppresses component/protocol/build/root-kind changes. A changed component or refreshed props disposes the old handle and calls `render`, never `hydrate`, after copying incoming metadata. Recreate validates incoming transport before any new setup; on failure it disposes the old handle, leaves the retained DOM inert and reports it instead of activating stale props. Apply this to both normal and skip-SSR persisted roots, overriding the old rule that skip-SSR roots never receive the remount flag.

Dev livereload discovers handles via the shared symbol, disposes old resources, and uses `render` for replacement with the new bundle's component code/identity. A repeated ordinary mount scan skips existing live roots; it does not dispose them. No private module map is authoritative across a re-import. Router events and existing data-zfb observation/persistence attribute names stay intact.

For initial skip-SSR, `data-when` applies exactly as for SSR roots: load, idle, visible and media are cancellable and recheck connection before firing; missing scheduling APIs use the existing immediate fallback. `mount` builds the new subtree off-DOM, validates it, then replaces fallback children in one step before activation. Setup/validation/cancellation failure leaves the fallback. A props/component recreate is already authorized lifecycle work and runs immediately in `render` mode, rather than awaiting a second initial scheduling trigger.

The stable definition witness for runtime-copy verification is the string `@takazudo/zfb/zudo-react/runtime-definition-v1`, emitted once as a property key on the shared reactive-core singleton. Description and protocol brands are not copy counts. #3291 counts **definition sites** of this witness across emitted entry/chunks (excluding source maps), not arbitrary string occurrences or its uses, and also proves cross-island shared-signal updates. The marker must remain observable in production output and not be optimized away; a quoted own property on the singleton retained through signal construction is the implementation requirement. A deliberate duplicated-copy negative control must make the definition-site count fail, even though both copies accept the same structural brands. Core import must not register browser globals or start scheduling. Structural brand agreement alone cannot prove one runtime instance.

## Temporary seams and cutover

ZR21 has two temporary seams, both removed by **#3288**:

1. **Framework/SDK seam (#3282, boundary extension #3284).** Config opt-in is exactly `framework: "zudo-react"`; adapter name is `zudo-react`; JSX source and server specifier are the owned subpaths above. The internal SDK factory specifier is `@takazudo/zfb/jsx-factory`, source `./src/jsx-factory.ts`, published `./dist/jsx-factory.js`/`.d.ts` with `types` and `default`. Its temporary default re-exports `Fragment`, `jsx`, `jsxs` through the existing Preact path; both bundlers alias only this owned factory specifier to `@takazudo/zfb/zudo-react/jsx-runtime` for the owned path. Source/package self-reference resolves the default under tsc; separate owned-path compile fixtures resolve a temporary typed owned implementation, not React typings, and exercise every shared SDK call. No alias from `react` or `preact` to zudo-react is permitted. The island boundary extension uses `@takazudo/zfb/island-boundary` with default `./src/island-boundary.ts` and owned `./src/island-boundary-zudo-react.ts` (published matching dist files); both bundlers substitute the owned implementation and compile fixtures check its interface. These are temporary integration subpaths, not sixth/seventh zudo-react public entries. #3288 replaces SDK imports with direct owned imports and deletes factory/boundary selector modules, export entries, aliases and temporary compile fixtures.
2. **Emitter seam (#3283).** `JsxDialect::ReactCompat` remains the interim default; `JsxDialect::ZudoReact` comes from the framework and travels on `Pipeline`. The non-default dialect is part of the compile fingerprint/cache key at every content and render-metadata call. Its Fragment import is `@takazudo/zfb/zudo-react/jsx-runtime`; generated props use `class`, HTML spellings, static `rawHtml`, and CSS style strings. Authored JSX is never rewritten. `render_md_page_shell` accepts the same dialect and uses `charset`. Existing default output stays byte-identical until cutover. #3288 removes the zfb framework-keyed dialect choice and page-shell default arm, so all zfb callers use the owned dialect. The legacy emitter arm and carrier remain solely for the separately scheduled md-wasm path until #3290, which removes that residual type/arm, the `jsxRuntime` option and its old goldens. The selectable zfb seam is removed by #3288; this bounded md-wasm tail is not another zfb runtime selector.

After #3288 there is no framework config key or selector, and a leftover key is a named migration error (L DD5). Delete `FrameworkAdapter` from `packages/zfb-runtime/src/framework.ts` and remove the framework argument of `createPageRouter`; the router imports `renderToString` directly from the owned server entry. Keep page-function awaiting and existing Response/string passthrough. The dead adapter hydrate shim and per-island bundle modes are not revived. The release and downstream consumer migrations remain outside this epic.

## Verification ownership and revisions

#3272 owns declarations/export maps; #3273 scopes/reactivity; #3274 server/transport; #3276 hydration/root; #3278 all form representations; #3279 structures. Until the latter two land, earlier implementation stages may explicitly reject their reserved props/built-ins with named not-yet-implemented errors; this is a sequencing state, not a final v1 omission. #3275 confirms R-A01 and logic R-A03 before those later stages, with later owners adding their exact-output cases.

#3277 supplies the packed-artifact browser harness; #3280 proves R-A02, browser/computed-style R-A03, R-A04–R-A08 and R-A10. #3281 owns lifecycle/boot composition; #3282–#3284 integration; #3291 proves R-A09/R-A11, scheduling/navigation, minified real output and both resolver paths. #3306 records real Japanese IME as deferred human verification. This document task runs only mechanical/formatter checks and a full foreground review. It makes no browser, visual, runtime-performance or real-build claim.

Revision 1.0.0 (2026-09-28): initial freeze. The explicit departures from provisional defaults are setup-only `getScope()` instead of a component's scope argument; `on:event` listener spelling instead of `onClick`; CSS-spelled object style keys; mandatory identity/name agreement; single-select only; uniform skip-SSR scheduling and remount handling; DOM-isolated `islandRoot` wrapper constructor plus explicit standalone island mode; structure-based ordinary-attribute tolerance under minification; removal of the router framework parameter; and the numeric flush/handle/definition-witness protocols specified above. DD8 also overrides the handoff's deferral of effects, rawHtml, select/radio and structural regions.

Revision 1.0.1 (2026-09-28): correct the writable-model type boundary required by ZR15 and #3278 scope item 10. `readonly value` in revision 1.0.0 did not prevent structural assignment from `ReadonlySignal<T>` to `Signal<T>`. Require the writable-only `$$zudoWritable: "zudo-react.writable.v1"` property on `Signal<T>` and `signal()` values, absent from `computed()`, with positive writable-to-readonly and negative readonly/computed model checks. #3278 owns the corresponding correction to the declarations/JSX types from #3272 and signal instances from #3273, plus runtime rejection and type regressions for all five adapters. Documentation/migration consumers must retain `Signal<T>` for writable model parameters and use `signal()` for models. This corrects type acceptance to the existing writable-model policy; transport/marker protocol version 1 is unchanged.

The lock revision is **1.0.1**. The planner addendum expands the lock set to #3272–#3291, #3297, #3298, #3300, #3303–#3305 and #3309. Additional direct consumers #3292, #3293, #3299, #3302 and #3306 receive runtime locks alongside their wind locks (32 runtime locks in total). Every lock preserves the issue header through its `**Task ` line and all text outside the inserted section. The manager coordinates shared-body edits.
