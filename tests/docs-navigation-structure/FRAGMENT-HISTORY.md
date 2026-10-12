# Fragment-history verification (#4167, #4165, #4157)

Scenario: page A, then a same-document fragment entry (`A#h`, by clicking the TOC link
and, separately, by assigning `location.hash`), a soft navigation to B, Back, Forward,
Back. Covers desktop 1440 and mobile 390, EN and JA. `fragment-history-verify.mjs` must
fail on the published runtime (null-state popstate ignored) and pass on the workspace
runtime carrying the #4165 fix. Run it against both hosts and keep both evidence sets.

Two subjects, built in disposable scratch worktrees of the integration SHA (never the
main checkout):

```sh
SHA=$(git rev-parse base/sweep-261012-reliability-navigation)   # or the exact integration SHA
EVD="$DROPBOX_CCLOGS_DIR/zfb/4167-evidence"; mkdir -p "$EVD"

# 1. Published baseline: normal docs build (zudo-doc 6.1.1, runtime 4.3.3 from the registry)
git worktree add --detach worktrees/scratch-published "$SHA"
(cd worktrees/scratch-published && pnpm install --frozen-lockfile && pnpm --filter docs build)

# 2. Changed source: workspace-built runtime packed into the docs host (writes provenance.json)
git worktree add --detach worktrees/scratch-ws-runtime "$SHA"
bash tests/docs-navigation-structure/build-workspace-runtime-host.sh \
  --scratch "$PWD/worktrees/scratch-ws-runtime" --evidence "$EVD/ws-runtime"
#   add --with-override if it reports more than one @takazudo/zfb-runtime copy
```

Check `provenance.json`: `installedEqualsWorkspaceDist` must be true,
`installedEqualsPublished` false, and `docsDistRouterAssets` non-empty. Compare the
`routerAssets` URL and sha256 recorded by the scenario against the docs/dist asset hash.

Serve and run (separate ports; run heavy steps under `heavy-guard.sh`):

```sh
(cd worktrees/scratch-published/docs && pnpm exec zfb preview --port 4401) &
(cd worktrees/scratch-ws-runtime/docs && pnpm exec zfb preview --port 4402) &

export DOCS_VERIFY_OUTPUT="$EVD"        # DOCS_VERIFY_BROWSER=webkit for the WebKit run
DOCS_VERIFY_LABEL=published    DOCS_VERIFY_ORIGIN=http://127.0.0.1:4401 node tests/docs-navigation-structure/fragment-history-verify.mjs   # expected: FAIL
DOCS_VERIFY_LABEL=ws-runtime   DOCS_VERIFY_ORIGIN=http://127.0.0.1:4402 node tests/docs-navigation-structure/fragment-history-verify.mjs   # expected: PASS
```

Evidence lands in `fragment-history-evidence-<label>.json` (per-case router lifecycle
log, served router asset URLs and hashes). A baseline that passes means the scenario
does not reproduce the bug; do not proceed on that result. Remove the scratch worktrees
afterwards (`git worktree remove`).

`verify.mjs` still carries the direct-load-with-hash workaround in `verifyBroaderTree`
("Back to an entry made by a same-document fragment navigation does not restore
content"). Remove it only after the ws-runtime run above passes.
