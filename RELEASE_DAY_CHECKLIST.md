## Release-day checklist (out of scope for this PR — flip manually)

Each package below has `private: true` and (for two of them) a
`*-migration.0` version string. Before publishing each one to npm:

- `packages/zfb` — bump `version` (currently `0.0.0`), remove `"private": true`, then `pnpm publish --access public`.
- `packages/zfb-runtime` — bump `version` (currently `0.2.0-migration.0`), remove `"private": true`, then `pnpm publish --access public`.
- `packages/zfb-adapter-cloudflare` — bump `version` (currently `0.1.0-migration.0`), remove `"private": true`, then `pnpm publish --access public`.

Sub 7b (this PR) prepared the surrounding metadata — descriptions,
keywords, repository / homepage / bugs / author / license / files
allowlists, `publishConfig.access: "public"`, READMEs as finished
npmjs.com landing pages, and CHANGELOGs — but did **not** touch
`version` or `private` per the issue body's explicit non-scope.
