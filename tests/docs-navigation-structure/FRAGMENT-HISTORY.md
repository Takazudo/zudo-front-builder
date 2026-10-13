# Fragment-history verification (#4167, #4165, #4157)

Scenario: page A, then a same-document fragment entry (`A#h`, by clicking the TOC link
and, separately, by assigning `location.hash`), a soft navigation to B, Back, Forward,
Back. Covers desktop 1440 and mobile 390, EN and JA. Each traversal must report the
matching router direction (Forward onto B is `forward`), and the same-page fragment entry and
a raw `?query` entry on the rendered page must neither swap nor fetch that document.

**Current expectation (#4184):** the docs host consumes the published runtime 4.4.0, which
ships the #4165 fix, so the normal docs build must **PASS**. On 2026-10-13 it passed on
Chromium and WebKit at source `b7d4158b` (served `islands-5e943541.js`; evidence in
`$DROPBOX_CCLOGS_DIR/zfb/4184-evidence/`). The mobile TOC variant is skipped because the TOC is
not visible at 390 px; the mobile hash variant runs.

**Historical (#4167):** the published runtime 4.3.3 ignored a null-state popstate and **failed**
this scenario, while the workspace runtime carrying the #4165 fix passed. The two-host recipe
below reproduced that comparison; it is kept for re-running an old tuple or a new workspace
runtime against the published one.

Two subjects, built in disposable scratch worktrees of the integration SHA (never the
main checkout):

```sh
SHA=$(git rev-parse base/sweep-261012-reliability-navigation)   # or the exact integration SHA
EVD="$DROPBOX_CCLOGS_DIR/zfb/4167-evidence"; mkdir -p "$EVD"

# 1. Published baseline: normal docs build (the runtime pinned in docs/package.json)
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
DOCS_VERIFY_LABEL=published    DOCS_VERIFY_ORIGIN=http://127.0.0.1:4401 node tests/docs-navigation-structure/fragment-history-verify.mjs   # 4.4.0+: PASS (4.3.3: FAIL)
DOCS_VERIFY_LABEL=ws-runtime   DOCS_VERIFY_ORIGIN=http://127.0.0.1:4402 node tests/docs-navigation-structure/fragment-history-verify.mjs   # expected: PASS
```

`DOCS_VERIFY_OUTPUT` is required (the script errors without it). Evidence lands in `fragment-history-evidence-<label>.json` (per-case router lifecycle
log, served router asset URLs and hashes). When reproducing the historical 4.3.3 failure, a
baseline that passes means the scenario did not reproduce the bug; do not proceed on that
result. Remove the scratch worktrees afterwards (`git worktree remove`).

`verify.mjs` still opens its broaden/restore scenario with a direct load of the article with
its hash in `verifyBroaderTree`. That load was a workaround for the 4.3.3 gap above; with the
published 4.4.0 runtime it is only the scenario's starting state, and fragment-history coverage
lives in `fragment-history-verify.mjs`. Replacing it with an in-page fragment entry is optional
follow-up, not a requirement.
