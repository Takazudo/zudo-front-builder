// Local type shim for the bare `zfb/config` specifier.
//
// `@takazudo/zfb` is consumed as a published npm package (version pinned
// in the root `package.json`). The package exposes its real config types
// under the *scoped* subpath `@takazudo/zfb/config` → `dist/config.d.ts`.
// But `zfb.config.ts` imports from the *bare* specifier `zfb/config`,
// which zfb's build tool aliases to a runtime-only stub at parse time
// (`zfb-config-stub.mjs` — `defineConfig` is identity, carrying no types).
// No real file backs `zfb/config` in `node_modules`, so this ambient
// declaration is what supplies the `ZfbConfig` type to `zfb.config.ts`.
//
// IMPORTANT — DO NOT hand-mirror `ZfbConfig` here. This block must stay a
// thin RE-EXPORT of the published `@takazudo/zfb/config` types. Earlier
// versions of this file copied the `ZfbConfig` shape field-by-field, which
// silently lagged the engine: every new config field had to be hand-added
// here or `zfb check` (plain `tsc --noEmit`) rejected a perfectly valid
// field with TS2353. That recurred twice — next.22 `bundle.exclude`
// (Takazudo/zudo-front-builder#678) and next.47 `codeHighlight.themeLight`
// / `themeDark` (#1073). Re-exporting the scoped package's types removes
// the hand-maintained field list entirely, so the shim can no longer fall
// behind — the whole "TS2353 on a newly-added valid field" class is closed.
//
// An ambient `declare module` wins over node resolution AND over tsconfig
// `paths`, so this declaration (not the package's own export map) is what
// `zfb check` binds `zfb/config` against. `export *` re-exports every named
// export of `@takazudo/zfb/config` — including the `defineConfig` value and
// the `ZfbConfig` type — into the bare specifier.

declare module "zfb/config" {
  export * from "@takazudo/zfb/config";
}
