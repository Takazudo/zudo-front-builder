# zfb owned engines — local development handoff

Prepared for the owner of zudo-front-builder on **2026-09-28 (Asia/Tokyo)**.

Target repository: [Takazudo/zudo-front-builder](https://github.com/Takazudo/zudo-front-builder).

Reviewed source: [`95d06aea3bc5b70b7f7df3253649673fef4da35a`](https://github.com/Takazudo/zudo-front-builder/tree/95d06aea3bc5b70b7f7df3253649673fef4da35a). The default branch was rechecked while preparing this handoff and still pointed to this commit. Local development must inspect its actual checkout; the document is a source-grounded plan, not a patch or a claim about future HEAD.

## Start here

The owner has approved **breaking Tailwind and React/Preact compatibility**. The owner is zfb's only current user; the new engines target the owner's consumers. Build two small engines around owned behavior and migrate those consumers directly. Keep esbuild.

1. Give your local agent [LOCAL-AGENT-PROMPT.md](LOCAL-AGENT-PROMPT.md).
2. Have it read the [decision record](docs/01-decisions.md), then inspect local repository instructions and current source.
3. Begin with the first zudo-wind vertical slice in the [delivery plan](docs/05-delivery-plan.md).
4. Use the verification cases to establish behavior before broad migration.

The comprehensive single-file edition is **HANDOFF.md**. It contains the same documents in reading order. The split files are easier for an agent to load selectively.

## Deliverable map

| File | Purpose |
| --- | --- |
| [LOCAL-AGENT-PROMPT.md](LOCAL-AGENT-PROMPT.md) | Copyable first instruction for the local agent. |
| [docs/01-decisions.md](docs/01-decisions.md) | Accepted user decisions, proposed defaults, scope boundaries, and compatibility policy. |
| [docs/02-zudo-wind.md](docs/02-zudo-wind.md) | Utility language, token model, compiler boundaries, source discovery, and CSS integration specification. |
| [docs/03-zudo-react.md](docs/03-zudo-react.md) | Setup-once JSX components, SSR, hydration, reactive bindings, forms, and ownership/lifecycle specification. |
| [docs/04-zfb-integration.md](docs/04-zfb-integration.md) | Source map and concrete migration points in the reviewed repository. |
| [docs/05-delivery-plan.md](docs/05-delivery-plan.md) | Ordered implementation phases, deliverables, and completion conditions. |
| [docs/06-verification.md](docs/06-verification.md) | Meaningful compiler, emitted-output, and real-browser acceptance cases. |
| [docs/07-sources.md](docs/07-sources.md) | Source citations, research limits, and facts the agent must verify locally. |
| [resources/decisions.json](resources/decisions.json) | Machine-readable decisions and implementation defaults. |
| [resources/acceptance-cases.json](resources/acceptance-cases.json) | Acceptance cases with IDs, evidence level, and expected behavior. These are specifications, not runnable tests. |

## Status and limits

- These are implementation instructions and design proposals. No zudo-wind or zudo-react implementation is included.
- Source inspection was performed; the replacements were not compiled, benchmarked, or run in a browser.
- No GitHub issue, branch, pull request, or release was created by this handoff task.
- The owner's downstream class/hook usage has not been exhaustively inventoried. That inventory is the local agent's first implementation task.
- `zudo-wind` is the owner-selected name. `zudo-react` is the working name from the conversation; it carries no React compatibility promise. Package paths and APIs below are implementation recommendations.

## What counts as success

`zudo-wind` produces the chosen site's CSS through a bounded, deterministic native library without invoking the Tailwind executable. The JSX runtime produces server HTML, adopts the existing browser DOM, updates explicit reactive bindings, and releases owned work on removal/navigation. The selected real consumer works under those owned contracts. esbuild continues to compile and bundle JavaScript.

Compatibility removal is authorized. Losing required user-visible behavior silently is not a completion criterion: implement it under the new contract, migrate the consumer deliberately, or document the specific remaining gap.
