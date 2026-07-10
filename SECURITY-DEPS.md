# SECURITY-DEPS.md — Runtime Dependency Audit

## Policy

Every runtime dep on a publishable package is a supply-chain liability for downstream users.
Add deps only when a Node built-in cannot do the job, document the reason here, and prefer
`pnpm dlx` / `npx` for build-time-only tools.

## Published packages and their runtime deps

The release pipeline publishes nine npm packages:

- five first-party platform binary packages: `@takazudo/zfb-darwin-arm64`,
  `@takazudo/zfb-darwin-x64`, `@takazudo/zfb-linux-arm64-gnu`,
  `@takazudo/zfb-linux-x64-gnu`, and `@takazudo/zfb-win32-x64-msvc`;
- four non-platform packages: `@takazudo/zfb`, `@takazudo/zfb-runtime`,
  `@takazudo/zfb-adapter-cloudflare`, and `create-zfb`.

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
- an optional `react` peer, used only by consumers that opt into React-facing APIs;
- `devDependencies` for local build/test/typecheck fixtures, including TypeScript, Vitest,
  `happy-dom`, `preact`, `react`, and type packages.

Those fields are intentionally separate from third-party production `dependencies`.

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

## Before adding a runtime dep

- [ ] Can a Node built-in (≥ 22) do this?
- [ ] Can this be a `devDependency` instead?
- [ ] Can this be `pnpm dlx`'d at build time?
- [ ] Is the dep on the OpenSSF best-practices list / has a recent release / has an active maintainer?
- [ ] Update this doc with the new dep + rationale.
