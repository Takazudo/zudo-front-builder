# SECURITY-DEPS.md — Runtime Dependency Audit

## Policy

Every runtime dep on a publishable package is a supply-chain liability for downstream users.
Add deps only when a Node built-in cannot do the job, document the reason here, and prefer
`pnpm dlx` / `npx` for build-time-only tools.

## Intended-public packages and their runtime deps

The three packages intended for publication are `@takazudo/zfb`, `@takazudo/zfb-runtime`,
and `@takazudo/zfb-adapter-cloudflare`. Their `dependencies` fields (NOT `devDependencies`)
are audited here.

### `@takazudo/zfb`

**Runtime deps:** none.

`@takazudo/zfb` has no `dependencies` field — only `devDependencies` (TypeScript, Vitest,
`@types/node`). All of those are build/test tooling that is never installed by downstream
users of the package.

### `@takazudo/zfb-runtime`

**Runtime deps:**

| Package | Version range | Rationale |
| ------- | ------------- | --------- |
| `hono`  | `^4.7.0`      | Hono is the page-router runtime — it is the entire point of this package. `zfb-runtime` wraps Hono's routing primitives to provide file-system-based page routing, content snapshots, and SSR for Cloudflare Workers. A Node built-in cannot replace a full HTTP routing framework. The dep is retained. |

### `@takazudo/zfb-adapter-cloudflare`

**Runtime deps:** none.

`@takazudo/zfb-adapter-cloudflare` has no `dependencies` field. The shipped CLI
(`bin/cli.mjs`) uses only Node built-ins (`node:fs/promises`, `node:fs`, `node:path`,
`node:url`) and one project-internal import (`../src/worker-wrapper.mjs`). No external npm
package is imported at runtime. The file carries an explicit `// invariant: no runtime npm
deps — see SECURITY-DEPS.md` comment to make this contractual.

## Audit policy

CI runs `pnpm audit --prod --audit-level=high`. `high`+ findings fail PRs. `moderate` and
`low` findings are intentionally informational — track them here, file issues for real fixes.
So "CI audit green" means no high/critical CVEs, not zero findings.

## Known moderate / low findings

_None at the time of this audit (2026-05-20). Update this section whenever a non-gated
finding is observed, with the advisory ID, affected package, severity, and the tracking
issue number._

## Before adding a runtime dep

- [ ] Can a Node built-in (≥ 22) do this?
- [ ] Can this be a `devDependency` instead?
- [ ] Can this be `pnpm dlx`'d at build time?
- [ ] Is the dep on the OpenSSF best-practices list / has a recent release / has an active maintainer?
- [ ] Update this doc with the new dep + rationale.
