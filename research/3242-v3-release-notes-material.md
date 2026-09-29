# v3 release-notes material

Release preparation input for the five package lanes. This is not a changelog page. The release owner supplies the version, date, computed sidebar positions, and final CI-measured artifact values when cutting the release.

## zfb

### Breaking changes

- **Configuration:** A 2.x `framework: "preact"` or `framework: "react"` key now fails with `the framework key was removed in zfb 3; delete the key. zfb now uses zudo-react.` Delete the key and migrate components to the owned runtime. Presets must be upgraded first because merged preset keys are validated. Evidence: `crates/zfb/src/config.rs` (`REMOVED_TOP_LEVEL_KEYS`, `reject_removed_top_level_keys`, `strip_presets`).
- **Configuration:** A 2.x `tailwind` key now fails with `the tailwind key was removed in zfb 3; utilities are compiled by the built-in zudo-wind engine. Replace tailwind: { enabled: false } with wind: false, or delete the key. See the v3 migration guide`. Use `wind` for generation or `wind: false` for authored CSS. Nested wind configuration rejects unknown fields; an absent `wind` key enables empty v1 settings with no tokens and reset `none`. Evidence: `crates/zfb/src/config.rs` (`REMOVED_TOP_LEVEL_KEYS`, `WindSetting`, `WindConfig`); `packages/zfb/src/config.ts` (`WindConfig`).
- **Styles:** Tailwind directives and imports, including imported sheets, fail with `ZW009` even with `wind: false`. Remove them, provide project tokens and an explicit reset where browser defaults matter, and replace unsupported utility syntax with authored CSS. No built-in palette or spacing unit is carried forward. Evidence: `crates/zfb-css/src/leftover_directives.rs` (`scan_leftover_directives`); `crates/zfb-css/src/css_imports.rs` (`bundle_authored_css`); `crates/zfb/templates/basic-blog/zfb.config.ts` (`wind`).
- **Styles:** zudo-wind uses a closed v1 language with complete candidate extraction and strict explicit candidates. Familiar Tailwind spellings do not guarantee the old result; missing tokens and breakpoints at strict origins fail, and ordinary class names emit no utility. Generated utilities are unlayered and ordered after the leading layer prelude and authored CSS. Evidence: `research/3242-zudo-wind-v1-spec.md` (W01–W29); `crates/zudo-wind/src`; `crates/zfb-css/src/pipeline.rs`.
- **Components and islands:** React and Preact runtime selection and compatibility are removed. Use `@takazudo/zfb/zudo-react` as `jsxImportSource`, HTML/CSS attribute spellings, native `on:event` listeners, setup-once components, and explicit writable `signal()` models. `<Island>` continues but requires one component child, strict JSON props, and valid source identity; hydration can fail closed per island. Evidence: `research/3242-zudo-react-v1-contract.md` (ZR01–ZR21); `crates/zfb/src/config.rs` (`REMOVED_TOP_LEVEL_KEYS`); `crates/zfb/templates/basic-blog/tsconfig.json`; `packages/zfb/src/island.ts`; `packages/zfb/src/zudo-react`.
- **CSS inputs:** Build and dev scan `.tsx`, `.ts`, `.jsx`, `.js`, `.mdx`, and `.md` for complete candidates. Dynamic concatenated prefixes are not inferred. Explicit standalone CSS sources can additionally be `.html` and `.mjs`; package roots and manifest files must be declared. Evidence: `crates/zfb/src/commands/css_support.rs` (`build_standalone_wind_source_plan`); `crates/zudo-wind/src/source_plan.rs`; `docs/src/content/docs/zudo-wind/sources-and-candidates.mdx`.

### Features

- `wind` configures tokens, breakpoints, dark mode, reset, safelist owners, authored classes, and producer-keyed manifests. `none`, `minimal-v1`, and `owned-v1` are the reset choices. Evidence: `crates/zfb/src/config.rs` (`WindConfig`); `packages/zfb/src/config.ts` (`WindConfig`); `crates/zfb/templates/basic-blog/zfb.config.ts`.
- `zfb css` is the standalone zudo-wind compiler with its explicit `--source` semantics; `zfb wind explain` and `zfb wind audit` inspect candidates. Evidence: `crates/zfb/src/cli.rs` (`CssArgs`, `WindCommand`); `crates/zfb/src/commands/wind.rs` (`explain`, `audit`).
- The owned JSX runtime ships through five `@takazudo/zfb/zudo-react` package entry points: core, JSX runtime, JSX dev runtime, server, and client. Evidence: `packages/zfb/package.json` (`exports`); `packages/zfb/src/zudo-react`.
- `zfb --version` reports the release version and embedded esbuild version. Evidence: `crates/zfb/src/cli.rs` (`long_version`).

### Removals

- Tailwind executable download, version pin, warmup and `ZFB_TAILWIND_BIN` / `ZFB_TAILWIND_OXIDE_WARMUP` overrides are removed; the version output has no embedded Tailwind line. Evidence: `crates/zfb/build.rs` (`embed_esbuild`); `crates/zfb/src/cli.rs` (`long_version`); `crates/zfb-toolchain-pins/src/lib.rs`; commit `0078751e9031d6c0f95cf9206379bcab2f352958`.
- React and Preact JSX selector paths, their package aliases, and the deprecated `ReactNode` consumer alias are removed. Evidence: `packages/zfb/src/jsx-types.ts`; `packages/zfb/package.json`; `crates/zfb-build/src/bundler.rs`; `research/3242-zudo-react-v1-contract.md` (ZR21).

## zfb-runtime

### Breaking changes

- Browser components and island hydration use zudo-react descriptions and scopes rather than Preact or React component reconciliation. Use writable `signal()` models, explicit reactive bindings, and native events; computed or readonly values cannot be form models. A changed component or props value recreates a render-mode root, while an unchanged persisted root remains. Evidence: `packages/zfb-runtime/src`; `packages/zfb/src/zudo-react`; `research/3242-zudo-react-v1-contract.md` (ZR09–ZR20).
- Islands require strict JSON props and scanner identity agreement; hand-authored wrappers require complete metadata. Adoption validates structure and ordinary text before committing, and an invalid island fails closed without taking down other roots. Evidence: `packages/zfb-runtime/src`; `research/3242-zudo-react-v1-contract.md` (ZR12–ZR19).

### Features

- Skipped SSR roots honor `data-when` and replace fallback atomically after successful client mount. Root handles use the shared root symbol, with independent root error reporting. Evidence: `packages/zfb-runtime/src`; `research/3242-zudo-react-v1-contract.md` (ZR19–ZR20).

### Removals

- The framework adapter selector and React peer dependency are removed. Evidence: `packages/zfb-runtime/src`; `packages/zfb-runtime/package.json` (`peerDependencies`); `research/3242-owned-engines-ledger.md` (DD2, DD3).

## zfb-adapter-cloudflare

- No package-specific changes.

## create-zfb

### Breaking changes

- Newly generated projects use zudo-react JSX and zudo-wind styles. Template `tsconfig.json` selects `@takazudo/zfb/zudo-react`; template `package.json` no longer installs Preact or React, and the old Tailwind import is absent. Existing generated projects must migrate their own files. Evidence: `crates/zfb/templates/basic-blog/tsconfig.json`; `crates/zfb/templates/basic-blog/package.json`; `crates/zfb/templates/basic-blog/styles/global.css`.

### Features

- The generated basic-blog project declares its palette, scales, dark selector, and `owned-v1` reset explicitly, with prose styling adjusted for the selected reset. Evidence: `crates/zfb/templates/basic-blog/zfb.config.ts` (`wind`); `crates/zfb/templates/basic-blog/styles/global.css` (`.prose`).

### Removals

- Generated `framework`, `tailwind`, Tailwind directives, and the old Preact JSX import source are gone. Evidence: `crates/zfb/templates/basic-blog/zfb.config.ts`; `crates/zfb/templates/basic-blog/styles/global.css`; `crates/zfb/templates/basic-blog/tsconfig.json`.

## zfb-md-wasm

### Breaking changes

- `compile()` and `renderHtml()` no longer accept the `jsxRuntime` option. Delete the option; typed calls reject it and JSON calls report an unknown field naming `jsxRuntime`. Evidence: `crates/zfb-md-wasm/npm/src/types.ts` (`ZfbMdWasmOptions`); `crates/zfb-md-wasm/tests/api.rs` (`compile` and `renderHtml` option rejection); `crates/zfb-md-wasm/npm/test/api.test.ts`.
- Compiled MDX now imports its Fragment from `@takazudo/zfb/zudo-react/jsx-runtime`. Consumers bundling compiled output must resolve that subpath. Evidence: `crates/zfb-content/src/mdx_jsx_emit.rs` (`_Fragment` import); `crates/zfb-md-wasm/tests/api.rs`.

### Features

- The owned JSX import-source family is used across MDX compilation and HTML rendering, with no runtime selector. Evidence: `crates/zfb-content/src/mdx_jsx_emit.rs`; `crates/zfb-md-wasm/src/lib.rs`; `research/3242-zudo-react-v1-contract.md` (ZR21).

### Removals

- The `JsxRuntime` public option/type and Preact/React output selector are removed. Evidence: `crates/zfb-md-wasm/npm/src/types.ts`; `crates/zfb-md-wasm/src/lib.rs`; `crates/zfb-md-wasm/npm/test/api.test.ts`.

The release owner must use the CI-measured values in `crates/zfb-md-wasm/shipped-sizes.json`; no local size number belongs in these notes. `ZFB_RELEASE_VERSION` is stamped into each `.wasm`, so all four SHA-256 digests change on release even if code and byte sizes do not. `release.yml` compares md-wasm sizes byte-exact. The next major version string is shorter than the current one, so sizes measured before release stamping can differ at release. Re-measure the manifest from a CI run carrying the release version **before** the release build. Evidence: `crates/zfb-md-wasm/shipped-sizes.json`; `crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md`; `.github/workflows/release.yml`; `CLAUDE.md` (Five-lane release changelog contract).
