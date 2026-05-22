# Node-Free Audit Report — Issue #379

**Date:** 2026-05-22  
**Branch:** `zfb-node-free-distribution/s1-audit`  
**Binary:** built from `crates/zfb` at this branch HEAD via `cargo build -p zfb --release`  
**Test project root:** `/tmp/zfb-audit-1779451613`

---

## Setup Notes

The test project used:

```
zfb.config.json    ← minimal `{}` (no plugins — intentionally absent; a non-empty `plugins`
                      array would invoke `plugin-host.mjs` via `node`, which is the component
                      this epic aims to make optional)
pages/index.tsx    ← minimal TSX page (no framework import)
pages/about.tsx    ← minimal TSX page (no framework import)
pages/index.md     ← plain markdown file (see finding below)
pages/page.html    ← plain HTML file (see finding below)
```

All three CLI commands were run as:

```sh
env -i PATH=/usr/bin:/bin HOME=$HOME ./target/release/zfb <subcommand>
```

This strips `node`, `pnpm`, and all other user-installed tooling from PATH, leaving only
`/usr/bin` and `/bin`.

---

## Command Outcomes

### 1. `zfb build`

**Exit code:** 0  
**Command:** `env -i PATH=/usr/bin:/bin HOME=$HOME zfb build` (from project root)  
**Output:**

```
⚠ scanned 2 page entries but found no "use client" islands; dist/assets/islands.js will not be emitted.
✓ 2 pages built in 1.12s
```

**dist/ output:**

- `dist/index.html` — rendered correctly (SSG, no framework, pure V8)
- `dist/about/index.html` — rendered correctly
- `dist/__zfb/routes.json` — populated with two routes
- `dist/assets/styles-*.css` — CSS emitted (Tailwind via embedded binary; no `node` needed)

**Result: GREEN** — `zfb build` works cleanly with stripped PATH and `zfb.config.json`.

> **Key observation:** The `.md` and `.html` files in `pages/` were silently ignored by the
> router. The scanner in `crates/zfb-router/src/scan.rs` (line 59) accepts ONLY `.tsx` files:
> ```rust
> if path.extension().and_then(|e| e.to_str()) != Some("tsx") { continue; }
> ```
> `.md` pages and plain `.html` pages placed in `pages/` produce zero routes today.
> See **Gap** section at the end.

---

### 2. `zfb dev`

**Exit code:** 0 (process killed after test)  
**Command:** `env -i PATH=/usr/bin:/bin HOME=$HOME zfb dev --port 3000` (from project root)  
**Startup output:**

```
⚠ scanned 2 page entries but found no "use client" islands; ...
→ ready on http://localhost:3000
```

**HTTP tests (via `curl`):**

- `GET /` → HTTP 200, correct HTML with livereload `<script>` injected
- `GET /about` → HTTP 200, correct HTML with livereload `<script>` injected

**Result: GREEN** — `zfb dev` starts, serves all pages, and requires no `node` on PATH.

Port 3000 confirmed closed after `kill $DEV_PID`.

---

### 3. `zfb new`

**Exit code:** 0  
**Command:** `env -i PATH=/usr/bin:/bin HOME=$HOME zfb new myproject` (from empty tmpdir)  
**Output:**

```
⚠ pnpm not found on PATH — skipping install. Run pnpm install manually before zfb dev.
✓ Created myproject (template: basic-blog). Next: cd myproject && zfb dev
```

**Scaffolded files:** All template files written correctly (`pages/`, `components/`, `content/`,
`layouts/`, `lib/`, `styles/`, `package.json`, `tsconfig.json`, `zfb.config.json`).

**Node invocation analysis** (`crates/zfb/src/commands/new.rs:292-307`):

- `zfb new` calls `try_pnpm_install(dest)` after writing template files.
- When `pnpm` is not found (i.e., `ErrorKind::NotFound`), it returns `PnpmOutcome::Missing`
  and prints the warning above. **Exit is still 0.** No hard failure.
- The scaffold itself (file writing, `package.json` patching) is pure Rust — no Node or npm
  is invoked during the write phase.

**Result: GREEN** (with note) — Scaffold completes without Node. The `pnpm install` post-step
degrades gracefully. This is **by design** (not a bug) and documented in new.rs.

**S2 guidance:** The scaffolded project (`basic-blog` template) still contains a `package.json`
with `preact` and `@takazudo/zfb` npm deps. A Node-free scaffold template (S2's goal) will need
to replace this with a `zfb.config.json`-only variant that does not reference npm packages.

---

## Node-Touchpoint Classification Table

| File | Touchpoint | Classification | Notes |
|------|-----------|----------------|-------|
| `crates/zfb/js/config-loader.mjs` | `node:module`, `node:process`, `node:url`, `node:path` | **C — Must keep Node as escape hatch** | Used only when `zfb.config.ts` (not `.json`) is present; `config.rs:1096` skips entirely for `.json` |
| `crates/zfb/js/plugin-host.mjs` | `node:process`, `node:readline` | **C — Must keep Node as escape hatch** | Spawned only when `config.plugins[]` contains entries with a `resolved_module` (the JSON path always sets `resolved_module = None`; see `plugins.rs:47-51`). Not invoked for JSON-only + no-plugin projects. |
| `crates/zfb/js/zfb-config-stub.mjs` | None | **A — Already pure Rust/pure JS** | Identity function for `defineConfig`; no Node APIs. Used as an esbuild alias when loading `.ts` config; irrelevant to the JSON path. |
| `packages/zfb/bin/zfb.mjs` | `node:fs`, `node:child_process`, `node:module`, `node:path` | **C — Must keep Node as escape hatch** | This is the npm-distributed shim that resolves and spawns the platform binary. Not part of the binary itself; used by `npx zfb` users before they have the binary. Not invoked at runtime by the Rust binary. |
| `packages/zfb-runtime/src/index.ts` | None | **A — Already pure Rust/pure JS** | Browser/worker-side runtime; no Node APIs. Runs inside V8 host. |
| `packages/zfb-runtime/src/router.ts` | None | **A — Already pure Rust/pure JS** | Hono-based page router; pure browser/worker surface. |
| `packages/zfb-runtime/src/framework.ts` | None | **A — Already pure Rust/pure JS** | Type interface for `renderToString`; no Node APIs. |
| `packages/zfb-runtime/src/snapshot.ts` | None | **A — Already pure Rust/pure JS** | Content snapshot type; no Node APIs. |
| `packages/zfb-runtime/src/view-transitions.ts` | None | **A — Already pure Rust/pure JS** | Browser View Transitions API wrapper. |
| `packages/zfb-runtime/src/client-router/` (all files) | None | **A — Already pure Rust/pure JS** | Entirely browser-side; `Node` references are DOM `Node` type, not `node:*` module. |
| `packages/zfb/src/content.ts` | `node:fs`, `node:path`, `node:module` (runtime-built specifiers) | **B — Needs a Deno port** | Reads `.md` content collection files from disk. Currently Node-only (`getCollection`). The `// TODO(zfb-content)` comment explicitly flags this for replacement once the Rust content engine ships. For a Node-free tier, this needs a Deno-compatible `fs` path or a Rust-side preloading strategy. |
| `packages/zfb/src/frontmatter.ts` | None (despite comment reference) | **A — Already pure Rust/pure JS** | Pure JS frontmatter parser; the comment notes it was deliberately separated from `content.ts` to avoid `node:fs` contamination. |
| `packages/zfb/src/config.ts`, `index.ts`, `island.ts`, `jsx-types.ts`, `paginate.ts`, `plugins.ts`, `runtime.ts`, `types.ts` | None | **A — Already pure Rust/pure JS** | Pure type helpers and definitions; no Node APIs. |

---

## Gaps Found

### Gap 1 — Router does not accept `.md` or `.html` pages (BLOCKS tier-1 MD+HTML path)

**Location:** `crates/zfb-router/src/scan.rs:59`  
**Code:**

```rust
if path.extension().and_then(|e| e.to_str()) != Some("tsx") { continue; }
```

Plain `.md` and `.html` files placed in `pages/` are silently skipped. The router accepts
only `.tsx` today. The spec for this epic calls for "an MD+HTML Node-free path", which
would require the router to handle at least `.md` files as first-class page sources.

**Impact:** This is a **tier-1 blocker** for the MD+HTML Node-free path. No `.tsx` file
(and therefore no TSX → JSX → V8 rendering pipeline) is needed for static HTML or rendered
Markdown output, but the router does not currently accept those file types.

**Suggested follow-up:** File sub-issue against epic #378 to add `.md` and `.html` route
scanning to `crates/zfb-router/src/scan.rs`, and a corresponding render path in the
build pipeline that does not invoke esbuild or V8 for pure-Markdown/HTML pages.

### Gap 2 — `packages/zfb/src/content.ts` uses Node-only `fs` API (BLOCKS content collections on Node-free tier)

**Location:** `packages/zfb/src/content.ts:265-310`  
**Classification:** B (Needs Deno port)

`getCollection()` loads `.md` files from the filesystem using `node:fs` loaded via a
runtime-built specifier. A Deno/WinterCG-compatible port would replace this with
`Deno.readTextFile` or an equivalent `Fetch`-based approach. The file already contains
a `// TODO(zfb-content)` marking it as a placeholder; the Rust `crates/zfb-content`
pipeline will eventually take this over.

### Gap 3 — Scaffolded template (basic-blog) still requires npm/Node for `pnpm install`

**Location:** `crates/zfb/templates/basic-blog/package.json`  
**Classification:** C for the current template

The `basic-blog` template scaffolds a Node/pnpm project. A separate Node-free template
(S2's scope) is needed for users who want a purely JSON-config + md/html project. S2
should produce a template whose `zfb.config.json` lists no plugins and whose pages require
no npm packages.

---

## Summary

All three commands (`zfb dev`, `zfb build`, `zfb new`) run successfully with
`PATH` stripped of `node` and `pnpm`. The `zfb.config.json` path (no `.ts`) skips
all Node subprocesses (esbuild + node config-loader, plugin-host). The embedded
esbuild and Tailwind binaries inside the `zfb` binary handle bundling without
external Node tooling.

The main tier-1 blocker identified is **Gap 1**: the router only accepts `.tsx` files,
so `.md` and `.html` pages are silently dropped. A follow-up sub-issue should be filed
to add `.md`/`.html` route support.

**STATUS: gaps** (Gap 1 blocks the MD+HTML tier-1 path as described in the epic spec)
