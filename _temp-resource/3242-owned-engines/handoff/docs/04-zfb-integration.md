# zfb integration and consumer migration

## 1. Authority, scope, and baseline

The owner explicitly accepts breaking compatibility with Tailwind, Preact, and React. The intended destination is an original `zudo-wind` utility language and an original, small `zudo-react` JSX runtime centered on SSR, hydration, explicit reactive bindings, and cleanup. Hooks compatibility is unnecessary. esbuild stays. Do not turn this project into a compatibility-layer implementation.

This source map uses the inspected zfb snapshot [`95d06aea3bc5b70b7f7df3253649673fef4da35a`](https://github.com/Takazudo/zudo-front-builder/tree/95d06aea3bc5b70b7f7df3253649673fef4da35a). Findings describe that snapshot, not an assurance that a later checkout has identical paths or behavior. Some repository comments describe historical designs and differ from the implementation; inspect executable code before relying on them.

Before implementation, read the current repository's `AGENTS.md`, `CLAUDE.md`, and applicable scoped instructions. Inspect the working tree and current HEAD. If HEAD has advanced, perform a focused comparison of the integration surfaces below and update the implementation plan. Preserve unrelated user work. Do not reset the checkout to this document's snapshot. This document authorizes no external publication, issue creation, message, or release; the current local development session determines those actions.

```sh
git status --short
git rev-parse HEAD
rg --files -g AGENTS.md -g CLAUDE.md
git diff --stat 95d06aea3bc5b70b7f7df3253649673fef4da35a...HEAD -- crates packages docs
```

Treat historical issue references as evidence explaining current machinery. They are not a queue of instructions to execute or a reason to resurrect previously removed behavior.

## 2. Recommended placement and dependency boundaries

Start inside the existing monorepo. Separate repositories and public package publication can wait until they solve a concrete distribution need.

| Proposed location | Responsibility | Dependencies it should avoid |
|---|---|---|
| `crates/zudo-wind/` | Candidate parsing, rule catalog, tokens, variants, deterministic ordering, diagnostics, and CSS generation. | zfb route orchestration, V8, Tailwind executable. |
| `crates/zfb-css/` | Adapt zfb source/configuration inputs to zudo-wind; combine authored CSS, CSS Modules, framework styles, and assets. | Browser component semantics. |
| `packages/zudo-react/` | JSX descriptors, signals/computed, SSR serialization, browser adoption/mounting, live bindings, and disposal. | zfb route scheduling, a Tailwind dependency, Preact internals. |
| Existing zfb renderer/islands packages | Select the runtime, supply server data, schedule activation, manage navigation, and bundle browser entries. | A new JavaScript compiler or module linker. |

Keep esbuild, SWC, embedded V8, and Lightning CSS. SWC's historically named React transform can target the new JSX import source; its name does not imply a React runtime dependency. Ship separate server and browser entry points for `zudo-react`, alongside `jsx-runtime` and the development JSX entry where the existing transforms need it. Select one canonical package specifier and use it consistently in generated imports, exports, type declarations, and embedded resolution.

Maintain a single reactive runtime identity within each JavaScript execution environment. SSR and browser naturally have separate instances; two accidental copies inside one browser graph must not split signal recognition, ownership state, or root tracking. Test the actual package resolution and emitted artifacts, not only source imports.

## 3. Source map: verified integration surfaces

All links in this table are pinned to the inspected snapshot.

| Surface | Existing implementation | Required integration work |
|---|---|---|
| CSS engine boundary | [`engine.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/engine.rs) defines `CssEngine`, Tailwind configuration, subprocess execution, and companion-asset transfer. | Add a zudo-wind adapter with deterministic inputs/results. Preserve or explicitly redesign asset provenance and dependency reporting. |
| Native CSS placeholder | [`native_engine.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/native_engine.rs) returns an unimplemented error. | Replace/remove the placeholder deliberately. It is no working engine to extend incrementally. Its old Tailwind implementation description is stale. |
| Authored CSS mode | [`authored_engine.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/authored_engine.rs) passes supplied CSS through without Tailwind. | Preserve an authored-CSS-only option under the new configuration. |
| CSS composition | [`pipeline.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/pipeline.rs) combines framework CSS, engine output, CSS Modules, hashes, and emission. | Keep one coherent output path; avoid implementing another asset emitter inside zudo-wind. |
| CSS module discovery | [`scanner.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/scanner.rs) discovers CSS Module imports. | Do not mistake this for an existing utility-class scanner. Implement candidate collection as its own concern. |
| Binary/runtime embedding | [`build.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb/build.rs) stages tool executables and framework/runtime packages. | Embed the new JSX package and later remove Tailwind-specific staging. Preserve esbuild and unrelated embedded dependencies. |
| Toolchain pins | [`zfb-toolchain-pins`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-toolchain-pins/src/lib.rs) pins esbuild and Tailwind and shares binary-source logic. | Remove obsolete Tailwind pins only after cutover. Shared platform/download decisions may still serve esbuild. |
| Renderer selection | [`adapters/mod.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-render/src/adapters/mod.rs) defines `Framework` and `Adapter`. | Add/select the new runtime; wire SSR entry and browser activation consistently. |
| Preact adapter | [`adapters/preact.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-render/src/adapters/preact.rs) installs SSR and hydration shims. | Use as a map of host integration requirements, not a semantics contract for zudo-react. |
| SWC JSX configuration | [`swc_pipeline.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-render/src/swc_pipeline.rs) independently defines `JsxRuntime`. | Update every selection path and development/production transform. |
| Island framework selection | [`bundler.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-islands/src/bundler.rs) independently defines `FrameworkKind`. | Remove unknown-runtime fallback to Preact for the new configuration; reject unsupported selections explicitly. |
| Generated island glue | [`esbuild.rs`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-islands/src/esbuild.rs) generates shared and per-island entries with framework-specific mount/unmount code. | Update both `render_shared_bundle_entry_source` and `render_island_entry_source`, plus JSX flags and resolution. |
| Island JSX wrapper | [`island.ts`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/packages/zfb/src/island.ts) imports `jsx` from `react/jsx-runtime`, extracts child type/props, and relies on aliasing. | Replace that concrete dependency and specify a zudo-react-compatible island boundary. Do not trust its older “agnostic/plain object” comments. |
| Browser lifecycle | [`runtime.ts`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/packages/zfb/src/runtime.ts) schedules islands, guards duplicate mounts, cancels pending work, and manages persistence/remounting. | Connect renderer disposal to the existing lifecycle; preserve behavior required by actual consumers. |
| Alternate hydration entry | [`hydrate.ts`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-islands/npm/src/hydrate.ts) uses an adapter shim. | Trace current callers to decide whether to update or retire this path. Changing only this shim does not update production entry generation. |
| SDK packaging | [`package.json`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/packages/zfb/package.json) includes JSX-related types, dependencies, export maps, and source/published configurations. | Migrate all applicable entry maps, type fixtures, peer dependencies, and packed artifacts. |

### CSS contracts beyond utilities

`CssEngine::take_package_url_companions()` transfers assets resolved from package-relative CSS URLs. A native generator that only returns utility declarations does not automatically preserve imported fonts/images. Design where import resolution and URL attribution happen, and return dependencies with their original source identity. The current trait can be revised: the owner permits breaking internal APIs, and a structured result can be clearer than mutable companion state.

Keep authored CSS, framework highlight styles, CSS Modules, public base URLs, and asset hashing in the established composition path. Define reset and layer behavior from the new specification. Pixel differences caused by deliberate style choices are acceptable; missing CSS or broken asset URLs are implementation failures.

The inspected docs stylesheet explicitly imports Tailwind preflight/utilities and zudo-doc styles, then uses `@source` and `@theme`. This makes the docs a useful integration case, but the external zudo-doc package itself was not audited here. Its imports, theme rules, and safelists require migration. [Docs stylesheet](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/docs/src/styles/global.css)

## 4. Find remaining cross-cutting locations locally

The following searches discover targets not exhaustively audited for this document. Read each relevant match before editing:

```sh
rg -n 'FrameworkKind|JsxRuntime|jsx_import_source|jsxImportSource|jsx-runtime' crates packages
rg -n 'TailwindSubprocess|NativeRustEngine|AuthoredCssEngine|CssPipeline' crates
rg -n 'tailwind|preact|react-dom|@types/react' --glob '*.{rs,ts,tsx,json,toml,yml,yaml,md,mdx,css}' crates packages docs scripts
rg -n 'framework|tailwind|schema' packages/zfb/src crates/zfb-config* crates/zfb/src
rg -n 'EMBEDDED_VENDOR|embedded_binary|resolve_alias|--alias' crates
rg -n 'transition-persist|island-remount|unmountIslands|mountNewIslands' packages crates
rg --files | rg 'template|scaffold|release|changelog|smoke|lock|schema|tsconfig'
```

Configuration needs a coherent update across Rust deserialization, TypeScript public types, schema validation, examples, CLI diagnostics, initialization templates, and any serialized config boundary. Use explicit engine selection during experiments. Existing `tailwind.enabled = false` users must have a defined authored-CSS-only destination; do not interpret it as enabling zudo-wind accidentally.

The new JSX source must agree across SWC, esbuild, MDX generation, package source builds, published builds, and embedded node-free execution. A local source fixture passing typecheck does not demonstrate that a packed consumer or embedded runtime resolves the same modules.

Do not globally alias `react` or `preact` to zudo-react. It has different semantics, and already compiled third-party packages may depend on their actual runtime. Remove aliases that served migrated first-party code only after tracing their remaining consumers. A truly isolated third-party widget can retain its own runtime behind an explicit DOM boundary; that is a consumer integration decision, not a requirement to build compatibility support into zudo-react.

## 5. Island contract and navigation

The proposed renderer should expose server rendering plus explicit browser hydration/mount and disposal. Adapt these into zfb's existing `mount(props, element, mode)` and `unmount(element)` convention, or revise that internal convention atomically. Store the renderer's root/disposer so unmount cleans subscriptions and listeners before detaching owned DOM.

SSR hydration must adopt existing nodes. Client-only mounting can replace its declared fallback. Keep these modes explicit. Data crossing the boundary should follow zudo-react's strict serializable-props contract; do not carry forward silent loss of unsupported values from old wrappers. Component identity must agree between the manifest and SSR marker. Do not depend solely on minifiable function names if the new design can use a build-assigned stable identity.

Navigation verification must cover deferred activation canceled before import completion, detached targets, repeated mount walks, hydration failure, and disposer failure. For persisted islands, retain both DOM identity and runtime scope when the persistence policy says state survives. A changed-props remount must dispose the prior scope exactly once before starting the new one. The browser scheduler and renderer must have one agreed owner of these transitions.

Mixed arbitrary VNode trees and cross-framework context are outside scope. Production selection is currently project-wide in the shared bundle path. Optional separate renderer roots require deliberate support; do not infer that adding an adapter automatically permits arbitrary mixtures.

## 6. Development sequence and cutover

1. **Inventory and fix the first specification.** Collect actual utility/variant/directive usage and active island features. Record intentional visual/API changes. Choose one representative consumer rather than assuming every sibling project is simple.
2. **Deliver one zudo-wind vertical slice.** Implement a bounded catalog, token input, diagnostics, candidate collection, authored CSS composition, and asset handling. Build real pages through zfb. Compare output and computed styles to approved expectations; Tailwind can serve as a temporary reference for adopted rules.
3. **Develop zudo-react independently.** Verify JSX execution once per instance, live signal/computed bindings, SSR escaping, DOM adoption, form policy, and scope disposal in focused fixtures. Keep zfb's existing renderer available while the new one is incomplete.
4. **Integrate real islands.** Wire framework selection, generated entries, wrapper data, shared runtime identity, and navigation lifecycle. Validate emitted bundles in a browser and the embedded SSR host.
5. **Migrate owned consumers and templates.** Port components, hooks, theme definitions, directives, package imports, and wrappers to the chosen new semantics. Rewrite small first-party wrappers where useful. Document unsupported third-party integrations explicitly.
6. **Switch defaults and remove obsolete machinery.** Delete the Tailwind executable path and unused React/Preact first-party adapters only when representative downstream builds and interactions pass. Update scaffolds, exports, lockfiles, embedded snapshots, tests, and release notes together.

zudo-doc, zudo-sg, and zzmod are candidate downstream projects, not audited compatibility certifications. Record which exact revisions and feature sets were exercised. No permanent Preact backend is required by this plan. Retaining it briefly to finish development is a transition tool; retaining it forever is a separate decision with continuing maintenance cost.

When adding a publishable package, inspect the current release instructions and package-set checks. The snapshot's [`CLAUDE.md`](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/CLAUDE.md) describes coordinated publishing and changelog lanes. Follow the current applicable rules when preparing a release; do not invent a new independent version channel as part of the engine prototype.

The go/no-go condition is correct rendering, asset resolution, activation, user input, and cleanup in selected consumers. Ecosystem compatibility is explicitly unnecessary. Ship a smaller documented contract when it works; do not expand the runtime merely to preserve old tests for behavior the owner has intentionally removed.
