# @takazudo/zfb-adapter-cloudflare

## 0.1.0-migration.0

Initial pre-release. Public surface:

- `getCloudflareContext<Env>()` — request-scoped accessor for the
  Cloudflare `env` and `ctx` bindings, backed by `AsyncLocalStorage`
  registered on `globalThis` under a stable key so the wrapper at
  `_worker.js` and the user bundle share a single instance even when
  they end up in separate ESM graphs.
- `@takazudo/zfb-adapter-cloudflare/build` — adapter entry consumed by
  `zfb build`. Emits `dist/_worker.js` (the Workers wrapper) and
  `dist/_zfb_inner.mjs` (the SSR bundle), ready for Cloudflare Pages
  advanced-mode deploy.
- `zfb-adapter-cloudflare` CLI — `zfb-adapter-cloudflare bundle <input.mjs> --outdir dist/`
  is the bin invoked by `zfb-build`. The wrapper imports the inner
  bundle by relative path instead of inlining it so the adapter does
  not need to ship an esbuild binary.

Package is workspace-internal pending the first public npm publish (see
release-day checklist in the repo root).
