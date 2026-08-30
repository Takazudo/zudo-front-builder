# JavaScript dependency-checker decision

## Recommendation

Do not wire a JavaScript dependency checker in #2752. If a checker is
revisited, pilot **Knip** rather than depcheck, in report-only mode first. Knip
has the better model for a pnpm workspace: it understands workspace projects,
entry points, exports, and the distinction between dependency categories. That
does not make its result authoritative without configuration, so it should not
become a required check until its findings have been reconciled with the
workspace's intentional peer-dependency contract.

## Why this workspace is noisy

The audit in #2746 found that the workspace is deliberately peer-heavy rather
than a collection of self-contained applications:

- `packages/zfb` and `packages/zfb-runtime` publish React-related peer surfaces;
  their devDependencies also provide test and typecheck consumers.
- `docs/` intentionally declares packages that its generated zudo-doc bundle
  consumes, including `mermaid`, `minisearch`, and `remark-cjk-friendly`, even
  where the imports originate in zudo-doc rather than in `docs/src`.
- The #2746 keep-list includes zudo-doc's optional peer dependencies (`diff`,
  `katex`, `zod`, `preact`, `@takazudo/zdtp`, and
  `@takazudo/zudo-doc-history-server`). Those declarations are opt-in feature
  support, not dead dependencies.

A checker that only compares source imports with manifest declarations can
therefore report both kinds of false positive: a docs-level declaration that
supplies an undeclared peer used by zudo-doc, and an optional peer that is
intentionally present for consumers. Generated bundles, package exports,
build scripts, and the publishable packages' peer/optional dependency
surfaces also need to be considered together.

## Knip versus depcheck

Depcheck is the weaker fit here. Its import-based heuristic is useful for a
small application, but it has limited awareness of a multi-project workspace,
package exports, generated entry points, and optional/peer dependency
contracts. It would need a broad ignore list before it could distinguish the
#2746 keep-list from actual dead declarations, which would make a green result
hard to trust.

Knip is the better future candidate because it can model workspace projects and
their entry points and can be configured for scripts, exports, and dependency
categories. It will still need explicit entry-point and ignore configuration
for the generated docs bundle and the zudo-doc peer contract; choosing Knip is
not a reason to suppress findings blindly.

## Promotion criteria

If this is revisited, pin a tested Knip release and run a non-blocking pilot
from the workspace root. Reconcile every finding against the #2746 inventory,
the two keep-lists recorded in `docs/CLAUDE.md`, generated build output, and
each publishable package's peer/optional dependency contract. Promote it to a
CI check only after two clean pilot runs and a reviewed configuration that
documents each intentional exception. Until then, the Rust cargo-machete
guard is the useful low-noise regression check; adding depcheck or Knip merely
for symmetry would create noise without a trustworthy signal.
