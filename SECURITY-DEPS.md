# SECURITY-DEPS.md — Runtime Dependency Audit

The workspace-wide dependency rationale is in
[DEPENDENCIES.md](./DEPENDENCIES.md). This file stays narrow: it audits the
runtime `dependencies` of published npm packages as downstream supply-chain
surface.

## Policy

Every runtime dep on a publishable package is a supply-chain liability for downstream users.
Add deps only when a Node built-in cannot do the job, document the reason here, and prefer
`pnpm dlx` / `npx` for build-time-only tools.

## Published packages and their runtime deps

The release pipeline publishes ten npm packages:

- five first-party platform binary packages: `@takazudo/zfb-darwin-arm64`,
  `@takazudo/zfb-darwin-x64`, `@takazudo/zfb-linux-arm64-gnu`,
  `@takazudo/zfb-linux-x64-gnu`, and `@takazudo/zfb-win32-x64-msvc`;
- five non-platform packages: `@takazudo/zfb`, `@takazudo/zfb-runtime`,
  `@takazudo/zfb-adapter-cloudflare`, `@takazudo/zfb-md-wasm`, and `create-zfb`.

Their `dependencies` fields (NOT `devDependencies`) are audited here. First-party
workspace links and optional platform packages still matter for publication integrity, but
they are not third-party runtime supply-chain surface in the same way a registry dependency
is.

### Platform binary packages

**Runtime deps:** none.

The five `@takazudo/zfb-<platform>` packages carry the built `zfb` binary, `README.md`, and
license metadata. They do not declare `dependencies`.

### `@takazudo/zfb`

**Third-party runtime deps:** none.

`@takazudo/zfb` does not declare a `dependencies` field. It does declare:

- `optionalDependencies` on the five first-party platform binary packages, so package managers
  install the matching native `zfb` executable when available;
- `devDependencies` for local build/test/typecheck tooling, including `@types/node`,
  TypeScript, Vite+ (`vite-plus`, which bundles Vitest), and `happy-dom`.

The SDK has no third-party production dependency or external framework peer. These
fields are intentionally separate from third-party production `dependencies`.

### `@takazudo/zfb-runtime`

**Runtime deps:**

| Package | Version source | Rationale |
| ------- | -------------- | --------- |
| `hono`  | `packages/zfb-runtime/package.json` | Hono is the page-router runtime — it is the entire point of this package. `zfb-runtime` wraps Hono's routing primitives to provide file-system-based page routing, content snapshots, and SSR for Cloudflare Workers. A Node built-in cannot replace a full HTTP routing framework. The dep is retained. Keep the package manifest and this rationale in sync when the range changes. |

### `@takazudo/zfb-adapter-cloudflare`

**Runtime deps:** none.

`@takazudo/zfb-adapter-cloudflare` has no `dependencies` field. The shipped CLI
(`bin/cli.mjs`) uses only Node built-ins (`node:fs/promises`, `node:fs`, `node:path`,
`node:url`) and one project-internal import (`../src/worker-wrapper.mjs`). No external npm
package is imported at runtime. The file carries an explicit `// invariant: no runtime npm
deps — see SECURITY-DEPS.md` comment to make this contractual.

### `@takazudo/zfb-md-wasm`

**Runtime deps:**

| Package | Version source | Rationale |
| ------- | -------------- | --------- |
| `@types/mdast` | `crates/zfb-md-wasm/npm/package.json` | The published TypeScript declarations expose mdast-compatible node shapes and import the ecosystem's `mdast` types. It remains a package dependency so a consumer can resolve the shipped declarations without relying on the producer's dev tree. |
| `mdast-util-directive` | `crates/zfb-md-wasm/npm/package.json` | The published `mdast.ts` declaration surface re-exports the directive type registry, preserving directive node typing for consumers. |
| `mdast-util-mdx` | `crates/zfb-md-wasm/npm/package.json` | The published declaration surface imports and re-exports MDX attribute and node types; consumers need the MDX type registry when using `parseToAst`/`MdastRoot`. |

These are declaration/runtime-contract dependencies of the published md-wasm
surface, not build-only tools. The parser and bundler test tools remain in
`devDependencies`.

### `create-zfb`

**Runtime deps:**

| Package         | Version source | Rationale |
| --------------- | -------------- | --------- |
| `@takazudo/zfb` | `packages/create-zfb/package.json` | Scaffold tool uses the first-party `@takazudo/zfb` package. pnpm rewrites the workspace specifier to the concrete package version at publish time. `@takazudo/zfb` itself has no third-party production `dependencies` field (see above), so `create-zfb` introduces no third-party transitive runtime surface. |

## Audit policy

There are two supply-chain audit lanes:

- **npm production deps:** PR CI and the weekly [`.github/workflows/security-audit.yml`](./.github/workflows/security-audit.yml)
  workflow run `pnpm audit --prod --audit-level=high`. `high`+ findings fail. `moderate`
  and `low` findings are intentionally informational — track them here and file issues for
  real fixes. "npm audit green" means no high/critical CVEs in production dependency graphs,
  not zero findings.
- **Rust deps:** the weekly security audit also runs `cargo deny check` using
  [`deny.toml`](./deny.toml). The `[advisories]` section is deny-on-finding for known
  vulnerabilities and yanked crates; licenses, bans, and sources currently report warnings
  rather than failing the job. The weekly workflow files or closes a tracking issue when
  either audit lane changes state.

Run the same checks locally with:

```sh
pnpm audit --prod --audit-level=high
cargo deny check
```

## Known moderate / low findings

_None currently recorded in this document. Update this section whenever a non-gated npm
finding or warning-only Rust finding is observed, with the advisory ID, affected package or
crate, severity, and the tracking issue number._

## Accepted cargo-deny exceptions

`deny.toml` ignores three Rust advisories. Takazudo accepted them as documented exceptions on
2026-10-21 ([#1373](https://github.com/Takazudo/zudo-front-builder/issues/1373)). None has a
fix reachable through the dependencies that pull it in, so each waits on an upstream release.
The reverse paths below were re-checked on 2026-10-21 with
`cargo tree --locked --target all -i <crate@version>`.

| Advisory | Crate | Reached via | Lift when |
| --- | --- | --- | --- |
| RUSTSEC-2025-0141 (unmaintained) | bincode 1.3.3 | `deno_core` 0.399.0, and `syntect` 5.3.0's `dump-load` feature behind `default-syntaxes`/`default-themes` | [syntect](https://github.com/trishume/syntect) releases without bincode 1. Removing `deno_core` (#4303) only narrows the path. |
| RUSTSEC-2024-0436 (unmaintained) | paste 1.0.15 | `v8` 147.4.0, through `deno_core`/`serde_v8` and directly through `zfb-js-host`. Every released `v8` up to 152.2.0 requires it. | [rusty_v8](https://github.com/denoland/rusty_v8) releases without paste. Removing Deno does not clear it. |
| RUSTSEC-2026-0235 (vulnerability) | rkyv 0.7.46 | `parcel_sourcemap` 2.1.1, through `lightningcss` 1.0.0-alpha.71's `sourcemap` feature, which `zfb-css`'s own `Bundler` use and `minify-html` 0.18.1's default features both require | [parcel_sourcemap](https://github.com/parcel-bundler/source-map) moves to rkyv ≥0.8.17, or lightningcss stops requiring it. The advisory needs a forged archive to be deserialized, and no identified zfb code path deserializes attacker-supplied rkyv archives through parcel_sourcemap. |

Do not bump crates or edit `Cargo.lock` just to chase these entries: a lock change needs its own
admission evidence. When an upstream trigger fires, remove the entry in the same change that
moves the dependency, and confirm `cargo deny check advisories` stays green. cargo-deny warns
`advisory-not-detected` when an ignore no longer matches anything, so a stale entry is visible
in the weekly Security Audit run.

## Before adding a runtime dep

- [ ] Can a Node built-in (≥ 22) do this?
- [ ] Can this be a `devDependency` instead?
- [ ] Can this be `pnpm dlx`'d at build time?
- [ ] Is the dep on the OpenSSF best-practices list / has a recent release / has an active maintainer?
- [ ] Update this doc with the new dep + rationale.
