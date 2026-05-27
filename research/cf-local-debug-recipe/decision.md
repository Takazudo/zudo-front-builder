# decision.md — CF Local Debug Recipe (Wave 2 / issue #539)

**Date:** 2026-05-27
**Decider:** Wave 2 (opus) reading `findings.md` from Wave 1 (#538) at SHA 90bfadb.

---

## Verdict

**Userland is sufficient. Do NOT add `zfb build --watch`.**

## Justification

Wave 1's empirical numbers clear the bar with margin:

- **Edit-to-curl latency 1.2–1.5s** across five consecutive edits (no outliers), well under any reasonable interactive-dev acceptance threshold.
- **D1 persistence verified** across 10+ rebuild ticks: `.wrangler/state/v3/d1/...` SQLite mtime unchanged, rows readable via `env.DB` after every reload.
- **Two consecutive edits** both produced fresh markers visible in the next curl response — no stale-module-cache regression on either side of the zfb/wrangler seam.
- The only added cost is two well-maintained devDeps (`concurrently@^9`, `chokidar-cli@^3`) — both already standard in the Node ecosystem.

A `zfb build --watch` flag would offer marginal UX gains (one fewer process, optional skip-CSS-when-untouched), but neither addresses a real user-blocking problem today. The complexity of adding a Rust-side file watcher with atomic-write guarantees is not justified by current friction.

## Decisions locked

1. **#540 (zfb `--watch` CLI):** **Skip.** Body annotated with `## Skip — Wave 2 chose userland` at the top. The downstream Wave 3.1 agent (if dispatched) immediately closes without coding.
2. **#541 (docs):** Locked the exact MDX replacement for the "Local development" section of `docs/src/content/docs/guides/ssr-and-cloudflare-bindings.mdx`. Covers one-time D1 migrate, the binding-realistic `dev:cf` loop, troubleshooting (stale `dist/`, port collision, the existing `nodejs_compat` Warning reference, `.wrangler/` D1 lifecycle, `pnpm dev` vs `pnpm dev:cf` incompatibility), and a short note distinguishing this loop from `zfb dev` (semantic parity only, no real bindings). The Japanese mirror at `docs/src/content/docs-ja/...` gets the same code blocks with translated prose.
3. **#542 (webshop):** Locked the exact `package.json` script entries (`dev:cf:setup`, `dev:cf`), the two devDeps to add (`concurrently@^9`, `chokidar-cli@^3` — both as devDeps so we drop the `npx --yes ...@N` runtime fetch the prototype used), the README replacement section, and the framework-pins.json bump *command* (not a static SHA) so the implementer captures the prototype branch's HEAD at execution time. Wave 4 does **not** depend on #540; it can proceed as soon as #541 lands.

## Branch-name note

The epic #537 body refers to the base branch as `base/cf-local-debug-recipe`, but the actual branch that exists in this repo is `cf-local-debug/prototype` (Wave 1 committed there and the task prompt's framework-pins lookup uses that name). #542's spec uses `cf-local-debug/prototype` consistently. If the manager intends to rename the branch before merging, the lookup command in #542 is the only thing that needs adjusting.
