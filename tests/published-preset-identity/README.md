# Published-preset runtime identity fixture (#3331)

A normal installed consumer of the **published** `@takazudo/zudo-doc` preset
composed with the **published** `@takazudo/zudo-sg` engine, built by the
**workspace-built** zfb binary. It proves that the presets' exports, plugin
hooks, injected package routes and virtual modules resolve one owned zudo-react
runtime through a real install, and that a second runtime copy is detected.

Required in the `health` job (`.github/workflows/health.yml`). Missing
packages, a failed install or a missing browser fail the run; nothing
self-skips.

```sh
ZFB_BIN="$PWD/target/debug/zfb" node tests/published-preset-identity/run.mjs
```

`ZFB_PRESET_EVIDENCE_DIR` overrides the evidence directory (default
`$RUNNER_TEMP` or the OS temp dir, `published-preset-identity/`). It receives
`evidence.json` plus every install, hook, and build log. Locally, wrap the run in
the machine-wide heavy guard.

## Layout

- `site/` — the consumer: exact-pinned `package.json`, frozen npm
  `package-lock.json`, `zfb.config.ts` (`withZudoSg(zudoDoc(...))`),
  `zudo-sg.config.mjs`, committed `zudo-sg` registry/token outputs, three docs
  pages, one catalog component, and the probe Island.
- `run.mjs` — the harness.

`site/` is not a pnpm workspace member and never touches the root
`pnpm-lock.yaml`. The harness copies it to a fresh directory **outside the
repository**, refuses to continue if any ancestor directory has a
`node_modules/@takazudo`, and runs `npm ci --ignore-scripts` against the
lockfile, so no workspace link, alias or hoisted fallback can satisfy an
import.

## Exercised surface

| Surface | How |
|---|---|
| Preset composition | `withZudoSg(zudoDoc({...}))` evaluated by the workspace binary |
| Plugin hooks | zudo-doc routes/search-index plugins and zudo-sg routes/preview-css plugins run during `zfb build`; `zudo-sg gen-registry --check` and `gen-token-manifest --check` run against the committed outputs |
| Package routes | zudo-doc's injected `/docs/[[...slug]]` renders all docs pages, including the probe; zudo-sg's `/components/[slug]` must emit `/components/counter` |
| Virtual modules | the server bundle must contain this site's `virtual:zudo-doc-route-context` and zudo-sg `sgContext` output; `chromeBindingsModule` reaches the package route through `virtual:zudo-doc-chrome-bindings` |
| One supported Island | `src/components/preset-probe.tsx` renders the package `ThemeToggle` (package-owned signals and activation) and the package `Details` with a host `computed` as its title, beside a host `<output>` showing the same value. Nested Island boundaries are rejected by the serializer, so the package components sit inside the one host Island. |

## Oracles

**1. Resolution and emitted identity** (`identityOracle`)

- Node resolution of `@takazudo/zfb/zudo-react` from the host root, from the
  installed zudo-doc package and from the installed zudo-sg package must reach
  one physical file, and the install must contain exactly one
  `@takazudo/zfb` package.
- The server bundle's sourcemap (`.zfb-build/bundle.mjs.map`) lists every
  module esbuild resolved. All zudo-react modules must come from one root, the
  bundle must hold exactly one runtime definition site
  (`"@takazudo/zfb/zudo-react/runtime-definition-v1": true`, the witness
  defined in `packages/zfb/src/zudo-react/reactive.ts`), and the probe-path
  modules (probe, boundary, chrome bindings, the package route source,
  `ThemeToggle`, `Details`) must be present.
- All emitted scripts under `dist/assets/` (searched recursively) together
  must retain exactly one runtime definition site.

The definition count alone is not the oracle: it is paired with resolution
evidence and with browser behavior.

**2. Real Chromium behavior** (`browserOracle`)

The server HTML shows `server:0:unset` in both the package-rendered `<summary>`
and the host `<output>`. After hydration both show `active:0:<theme>` and the
`ThemeToggle` drops `data-zd-pending`. Clicking the host button updates both
portions. Choosing **Light** in the package menu updates the package button's
label and, through the package's preference event, the host portion. A
client-router navigation to another package-routed page disposes the Island
(cleanup ledger `1/1`, same document). Going back remounts a live Island
(`2/1`). Any page error, console error or console warning fails.

## Negative controls (expected failures)

The harness passes only when each control makes its targeted assertions fail.
A liveness timeout or unrelated build error does not count.

| Control | Change | Required failure |
|---|---|---|
| `negative-nested-package-copy` | A second copy of the installed `@takazudo/zfb` is nested at `node_modules/@takazudo/zudo-doc/node_modules/@takazudo/zfb`, so zudo-doc's own imports bind to it while the host keeps the owned one | Each named check in `evidence.json` (`checks`) must hold: zudo-doc resolves a different zudo-react file; two installed runtime copies; the build fails closed with `ZR_NO_SCOPE: getScope requires synchronous setup` because package components call `getScope()` on the copy. The render fails after bundling, so the server bundle must be emitted and show two server runtime roots and two server definition sites, and no client assets may be emitted (`bundleArtifacts` records both) |
| `negative-host-mismatched-copy` | The installed `@takazudo/zfb` is copied to `@fixture-negative/zfb-copy`, and only the probe's `signal` import is switched to it | Identity oracle fails (two client and server definition sites, two server roots), **and** the browser oracle fails in the targeted way, recorded as named checks: the Island mounts, the hydration step fails, and both the package `<summary>` and the host `<output>` still read `server:0:unset` because the host `computed` from the owned runtime cannot track the copy's signals. Any other browser failure does not count |

## Provenance: installed packages vs workspace compiler

- **Installed from npm (lockfile, registry integrity):** the presets and every
  dependency, including `@takazudo/zfb`, `@takazudo/zfb-runtime` and
  `@takazudo/zfb-md-wasm` at the pinned engine version. The presets import
  them by bare specifier, exactly as a consumer would.
- **From the workspace:** the `ZFB_BIN` binary, which is the compiler, the
  renderer, and the runtime's source of truth. It embeds the workspace's
  `packages/zfb` and `packages/zfb-runtime` TypeScript sources and stages them
  into its build shadow. Its bundles therefore resolve `@takazudo/zfb/*`
  to that embedded source (sourcemap root `node_modules/@takazudo/zfb/src`),
  not the installed package's `dist/`. It does the same for each negative
  control's copy, which is why those copies show up as extra `.../src` roots.
  The installed `@takazudo/zfb` still
  matters because package resolution, typing and nested-copy detection go
  through it.
- The npm package's own native carrier binary (`@takazudo/zfb-<platform>`) is
  installed but never executed. `evidence.json` records `ZFB_BIN`'s path,
  SHA-256, `--version` and mtime, plus the checkout SHA. In CI the binary is
  the one `health` just built from that SHA; a local binary may predate the
  checkout.

## Updating the package versions

1. Edit the exact versions in `site/package.json`. Keep the engine tuple
   (`@takazudo/zfb`, `zfb-runtime`, `zfb-md-wasm`) in lockstep and satisfy the
   presets' peer ranges (`npm view @takazudo/zudo-doc@<v> peerDependencies`).
2. Regenerate the lockfile in a scratch copy outside the repository so no
   workspace state leaks in:
   `cp -R tests/published-preset-identity/site /tmp/ppi && (cd /tmp/ppi && npm install --ignore-scripts --no-audit --no-fund)`,
   then copy `/tmp/ppi/package-lock.json` back. Every entry must resolve
   from `https://registry.npmjs.org/` with an integrity hash; the harness
   rejects anything else.
3. In that scratch copy run `npx zudo-sg gen-registry` and
   `npx zudo-sg gen-token-manifest`, and copy any changed
   `src/styleguide/*.ts` back.
4. Run the harness. If a preset changed its public surface (component names,
   route paths, the `data-zd-pending` contract or menu labels), update the probe
   and the oracle together, and keep both negative controls failing for the
   reason stated above.
