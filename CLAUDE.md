# zfb / zudo-front-builder

Workspace for the `zfb` Rust workspace + `@takazudo/zfb-runtime` and `zfb` npm packages.

## /x-wt-teams epic workflow rule

For each `[Epic]` issue in this repo (e.g. issues #52, #53, and the rest of the super-epic's zfb-side epics), the epic PR is independent and safe to merge into `main` as soon as its workflow completes — siblings do not stack on one base. So:

- **Always invoke `/x-wt-teams` with `-a` / `--auto`** for an epic in this repo (in addition to whatever other flags the user passes — `-gcoc`, `--stay`, etc.). `-a` runs `/pr-complete -c -w` after Step 15, which merges the root PR, closes the linked issue, and watches post-merge CI on `main`.
- **After the merge succeeds**, the workflow's auto-suggest step prints the next epic's `/x-wt-teams` command for the user to copy-paste. Do not start the next epic in the same session.
- If the user explicitly says "do NOT auto-merge" or passes a flag that conflicts with `-a`, defer to the user.
- This rule is specific to `[Epic]` issues. Non-epic issues (one-off bug fixes, follow-ups like #58 inside an open epic PR) keep the default behaviour — leave the PR open for the user to review.

## Worktree push policy (enforced)

This repo uses `/x-wt-teams` for multi-topic development. Child agents work in git worktrees under `worktrees/`. **Pushing from a worktree is forbidden.** Only the manager session — running from the main repo at the repo root — pushes, after merging topic branches into the base branch locally.

### Why

- CI runs on every push. Children pushing pre-empt the manager's merge + review step, multiplying CI cost across intermediate state.
- Topic branches in `worktrees/*/` are intermediate by design — they shouldn't appear as standalone PRs unless the manager creates them.

### How it's enforced

`.git/hooks/pre-push` is a direct script (not managed via `lefthook.yml`) that blocks any push from a git worktree. It is auto-installed by `pnpm install` (via the `prepare` lifecycle script) and can be re-installed manually with:

```sh
pnpm init-worktree
```

The installer source lives at `scripts/install-git-hooks.sh`; the hook itself at `scripts/hooks/pre-push`.

### Emergency bypass (human use)

```sh
ALLOW_WORKTREE_PUSH=1 git push ...
```

Use only when you genuinely need to push from a worktree (rare). Never set this in agent prompts.

### Guidance for agents

- **Child agents working in `worktrees/*/`:** commit locally only. Pushing will fail with the message above — do not retry, do not invoke the bypass. Report back to the manager with the branch name and commit SHAs; the manager merges and pushes from the main repo.
- **`/x-wt-teams` manager session:** the hook does not affect you. Your `git push` runs from the main repo (the cwd is the repo root, not `worktrees/...`). After every wave's local merges, push as usual. Do not pass `ALLOW_WORKTREE_PUSH` to children.
