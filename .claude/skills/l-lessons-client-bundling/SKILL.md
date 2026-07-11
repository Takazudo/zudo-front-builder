---
name: l-lessons-client-bundling
description: "Project lessons learned for client bundling (zfb-build bundler, bundle.exclude, shadow staging, esbuild resolution) and for running autonomous review→fix loops on this area. Read PROACTIVELY before planning or implementing work touching crates/zfb-build/src/bundler.rs, bundle.exclude semantics, or any fully-autonomous epic run — contains traps, root causes, and \"watch for next time\" notes from previous attempts."
---

# Lessons: client bundling

## 2026-07-11 — bundle.exclude vs esbuild resolution: non-converging autonomous fix loop

### What we set out to do

Epic #1497 (client bundling primitives: bundle-mode loaders/defines, raw imports, module workers, `bundle.exclude`) via a fully autonomous Codex `x-wt-teams` run with `-m -a` (auto-fix findings, auto-merge, no human checkpoint).

### Approach we tried first

Two compounding choices:

1. **Code:** make `bundle.exclude` airtight by *predicting in Rust what esbuild would resolve* — progressively reimplementing esbuild's resolver (extension probing, index files, package.json `main`/`imports`, node_modules walk, symlink semantics, Go `filepath` quirks), version-pinned to esbuild 0.25.12.
2. **Process:** review→fix→re-review rounds with the same agent family on both sides, no round budget, no convergence criterion, no same-seam detection, no human escalation.

### Why it went wrong (root cause)

Two unbounded loops multiplied. A correctness criterion of "matches what esbuild would do" has **no finish line**: every fix expands the parity surface, so every review round finds a genuinely new, empirically-valid divergence — the reviewer is right every time, and that is exactly why it never converges. The harness then had no brake: 6 escalating fix commits in 3h39m (+366 → +372 → +284 → +125 → +953 → +1986/−354 lines, all in `crates/zfb-build/src/bundler.rs`), two merges both labeled "final", then a round-8 worktree with +5,202 more uncommitted lines. One helper (`concrete_tsconfig_target_is_excluded`) was rewritten 5 times in ~3h and then deleted; another function lived 21 minutes.

### What worked instead

- **Interrupting and diagnosing** instead of another round. The committed state already passed all its own tests (42/42 integration, 126/126 unit); the remaining "findings" were parity-surface artifacts.
- The architecture the final commit landed on is the keeper: **exclusion = absence from a staged shadow tree; Rust collects candidate SETS but never picks winners — esbuild remains the only resolver**, choosing inside an isolated shadow.
- The not-yet-adopted fixed point the loop was circling (candidate simplification for future work): when `bundle.exclude` is non-empty, suppress *every* live-tree fallback (dual-target tsconfig paths, node_modules symlink escape) so exclusion means nothing more than absence-from-shadow; optionally backstop by auditing esbuild's `--metafile` `inputs` (its authoritative resolution record, already parsed in the codebase) for excluded paths. That deletes the 3-state exclusion analysis and the prefix-overlap heuristics.

### Watch for next time

- If a change's correctness criterion is "matches what esbuild (or any external tool) would resolve/do", you are reimplementing that tool — unbounded. Restate the spec so the tool itself is the oracle (absence-from-shadow, metafile audit) or restrict semantics (e.g. exclusions apply only under walked source roots, hard config error on alias-target overlap — the knob's actual #664 use case).
- If review rounds keep flagging the same file/seam and each finding is *new but valid*, the architecture is the bug, not the findings. Stop after 2 rounds on the same seam and escalate the design question instead of fixing again.
- Escalating fix sizes are a divergence signal: a converging fix sequence shrinks (+366 → +50 → +5), not grows (+366 → +953 → +1986). Track it.
- A second "final" review/merge after a first "final" merge is a loop tell — stop the run.
- Autonomous runs (`-a`/`-m`, auto-fix defaults) need an explicit round budget baked into the workflow skill; without one, agent-reviews-agent runs indefinitely and never asks a human the architectural question.
- Known live-tree escape hatches that can resurrect an "excluded" file: dual-target rebased tsconfig `paths` (`[shadow, real]` fallback), the `<shadow>/node_modules` symlink to the live dependency tree, and esbuild candidate probing silently substituting a sibling (`foo.js` when `foo.ts` is excluded, `foo/index.ts`, a package `main`).
- zfb drives the prebuilt esbuild *binary* as a subprocess under the node-free-at-build-time constraint (build-no-v8 CI lane) — there is no `onResolve` plugin interface. Don't plan a plugin-host solution; it's a product-scale change.
- Worktree push policy means an autonomous local loop leaves **no GitHub trail** — a PR can look untouched (0 changed files) while 38 commits and hours of churn exist locally. Check `git reflog` and agent session logs, not just the PR.

### Would-skip-if-redoing

The entire predict-the-resolver phase (loop commits 2–5): five rewrites of a single predicate, an `exports`-field walker deleted 21 minutes after creation, a byte-for-byte revert of an extension-list change. The final commit's staging/absence architecture was reachable directly from the first finding, had the design question been asked instead of patch-iterated. Also skip: round 8 (+5,202 uncommitted lines chasing 3/63 remaining tests down the same parity hole).
