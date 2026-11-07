# esbuild after the removal milestone (#4628)

The milestone removed esbuild from what zfb owns: the shipped executable, build-time vendoring
and download, every bundling path (page/SSR, islands, standalone client scripts, module workers,
TS config, TS-family plugins), the alternative backend and its options, and the active
first-party compile/test helpers. zfb now bundles only with the embedded native Rolldown
(`crates/zfb-bundler`, identity `rolldown-24bc2d0`).

This is **not** a claim that the repository's dependency tree is esbuild-free. Third-party
tools still bring esbuild in, and a few legacy or verification-only files still name it. This
page lists them with owners so they are not mistaken for zfb product dependencies.

Inventory taken at `base/esbuild-removal` `f44a276` from `pnpm-lock.yaml`, `pnpm why esbuild -r`
and a source search.

## No published zfb package depends on esbuild

`pnpm --filter <pkg> why esbuild --prod` finds no esbuild for any of the published packages:
`@takazudo/zfb`, the five `@takazudo/zfb-<platform>` carriers, `@takazudo/zfb-runtime`,
`@takazudo/zfb-slugify`, `@takazudo/zfb-adapter-cloudflare` and `create-zfb`. The scaffold
templates in `crates/zfb/templates/` declare no esbuild, wrangler or tsx dependency.

## Third-party transitive esbuild (development and docs only)

| esbuild | Brought in by | Used for | Owner |
| --- | --- | --- | --- |
| 0.27.3 | `wrangler@4.85.0` (root and `docs` devDependencies) | Cloudflare deploys of the docs site and showcase, adapter tests | Cloudflare (upstream); zfb maintainers own the pin |
| 0.27.7, 0.28.1 | `@voidzero-dev/vite-plus-core@1.0.0` optional peer, through `vite-plus`, `vitest` and `@vitest/*` | The JS test runner (`vp test`) | VoidZero (upstream) |
| 0.28.1 | `tsx@4.23.1`, a vite-plus-core peer | TS execution inside the test tooling | tsx (upstream) |
| 0.28.2 | `@takazudo/zudo-doc@9.1.2` and its `tsx@4.23.15` (`docs` dependencies) | The docs site generator | Takazudo (zudo-doc) |

`pnpm-workspace.yaml` keeps `allowBuilds: esbuild: true` because those packages still install
esbuild's native binary. Removing it would break wrangler and the test runner, not zfb.

## Legacy and verification-only first-party references

| Location | What it is | Owner / next action |
| --- | --- | --- |
| `tests/published-browser-verification/run.mjs` | `@verification` harness for #3879. It installs historical *published* zfb versions and bundles a consumer page with npm `esbuild@0.25.11`. No workflow runs it. | Manager-owned historical receipt. Replace the consumer bundling with zfb itself, or retire the harness, if it is ever rerun. |
| `crates/zfb-md-wasm/npm/test/fixtures/vite-6.4.3/` | Consumer-compatibility fixture. Vite 6 depends on esbuild, and the fixture sets `esbuild: false` for builds. | zfb-md-wasm compatibility. Third-party by design. |
| Metafile `Spelling::Esbuild` and timing phase names (`main-esbuild-bundle`, `phase=esbuild`, `esbuild=`) | Output-format and log-key names that scripts parse | Rename together with their parsers in a follow-up. Not an esbuild dependency. |
| Test names and comments in `crates/**` and `packages/zfb-runtime/src` | Historical wording | Cleanup follow-up. |
| Darwin arm64 size contract (`research/v3-island-size/decision.json`) | Still schemaVersion 3, recording `esbuildVersion 0.25.12` | Re-pin from owner-native Mac evidence, plus a Darwin bundler-backend re-pin reason (downstream). |
| Changelogs, release notes and `research/**` history | Historical record | Kept unchanged on purpose. |
