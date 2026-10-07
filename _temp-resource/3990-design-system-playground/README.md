# Accepted design-system playground handoff — issue #3990

Specification: https://github.com/Takazudo/zudo-front-builder/issues/3990

The maintainer accepted UI round 2 on 2026-10-07. This directory preserves the original interactive prototype, source, screenshots, seed exports, integration notes and accepted implementation brief for Codex Cloud.

## Archive integrity

- File: `zfb-design-system-codex-handoff.zip`
- Bytes: 645358
- SHA-256: `2513f04a71081b3dfd497e7d94370bcc39285fbd609953c17ec015dd2ff1c421`
- ZIP entries: 54

Extract the archive into an ignored temporary working directory. Start with `zfb-design-system-codex-handoff/ACCEPTED-SPEC.md`, then `prototype/zfb-design-workshop-r2.html`, `prototype/source/`, `prototype/integration/README.md`, and `prototype/QA-NOTES.md`. Paths after the first are relative to the extracted handoff root.

The full specification also lives in the issue. This resource supports implementation; unavailable optional prototype resources must not block independent work that the issue fully specifies.

## Continuation and cleanup

The task branch is `base/design-system-playground-r2`, prepared from `main` at `09a9f9b38f2aae922ebd32128fcfb9a603ba7318`. Reuse its draft PR as the implementation PR targeting `main`. This initial commit contains resources only; it does not implement or verify the native documentation or bundled starters.

Implement the actual docs playground and both Basic blog / Node-free starter improvements. Keep docs-host migration #3329 separate and the production initializer selector deferred. Move lasting source, docs and tests into their production paths, then remove this entire `_temp-resource/3990-design-system-playground/` directory from the final implementation diff before merge.

Prototype browser and archive checks passed as described in QA-NOTES. Native ZFB docs integration/build/hydration and target Wind compilation have not been performed. The receiving session must validate those against the actual implementation revision.

Implementation, scoped checks, commits, task-branch pushes and updating the prepared PR are authorized. Merge, deployment and package publication are not. Leave issue #3990 and the implementation PR open for review. Do not merge this resources-only delivery.

No private ChatGPT plugin, prior chat history or private hosted-preview access is required.
