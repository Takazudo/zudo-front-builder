# 01 — Decisions and scope

## 1. Owner-approved decisions

These decisions come from the conversation and are authoritative for the handoff.

| ID | Decision | Consequence |
| --- | --- | --- |
| D01 | The owner is zfb's only current user; the new engines target the owner's consumers. | Optimize the contract for the owner's applications and development workflow. |
| D02 | Dropping Tailwind compatibility is explicitly acceptable. | No promise to accept the whole Tailwind language, plugin system, default theme, or future changes. |
| D03 | Build `zudo-wind` with an independently fixed specification. | Adopt individual utilities/ideas on the owner's timing; version the owned behavior. |
| D04 | Dropping React/Preact compatibility is explicitly acceptable. | No required hooks, component class, context, VNode, or third-party component emulation. |
| D05 | JSX/component authoring and hydration are the essential runtime goals. | Keep composition, server HTML, and actual adoption of existing DOM. |
| D06 | A small built-in rendering model is preferred. | Favor explicit lifetimes and a bounded API over generalized extension points. |
| D07 | esbuild stays. | Do not migrate to Rolldown, replace the linker, or invent another JS/TS parser/compiler in this project. |
| D08 | Implementation continues with a local AI agent. | This package is a concrete handoff, not an implemented or deployed engine. |

The final owner clarification was: **“dropping the compatibility is perfectly ok”**, because the sole user is the owner. Do not reintroduce compatibility as a required gate merely because old project documents or dependencies assumed it. The approval concerns this design decision; it does not cancel unrelated repository workflow or verification instructions.

## 2. Recommended implementation defaults

These defaults make the plan executable. The local agent may revise them based on current source or real consumer needs, recording the reason. They are not additional questions that require repeated owner approval.

| ID | Recommended default |
| --- | --- |
| P01 | Implement zudo-wind as a native Rust library initially inside the zfb workspace (`crates/zudo-wind`). |
| P02 | Reuse the existing CSS/asset pipeline and Lightning CSS where appropriate; replace the utility generation boundary rather than duplicating the entire CSS pipeline. |
| P03 | Treat Tailwind v4.2.0 as a pinned reference for selected inherited behavior, not the definition of all supported behavior. |
| P04 | Use explicit source plans and safelists; make incremental removals and dependency invalidation deterministic. |
| P05 | Put the JSX runtime in `packages/zudo-react` initially, with JSX, server, and browser entry points and no routing responsibilities. |
| P06 | Use setup-once function components and explicit reactive values. Signal/computed objects are live in JSX; plain values are initial snapshots. |
| P07 | Give each island/component/removable region a disposal scope and explicit browser activation callbacks. |
| P08 | Adopt DOM with deterministic boundaries and structural validation; report mismatches instead of silently remounting. |
| P09 | Start with static composition, reactive text/attributes/properties, native events, and form ownership. Add structural regions only as actual selected consumers require. |
| P10 | Develop the engines separately. Reach a real CSS vertical slice first; integrate the JSX engine after its browser contract is proven. |

`zudo-react` is a working name used in the discussion. The name does not promise React compatibility. Final npm naming, public publishing, separate repositories, and a long-term package distribution model are not settled requirements; local workspace packages suffice to begin.

## 3. Superseded proposals

- **Mandatory embed-Tailwind-first phase:** superseded by the owner's preference for owning the language directly. Embedding the Tailwind compiler in V8 remains a researched fallback if a concrete need emerges, but is not a dependency of this plan.
- **Preserve Preact semantics behind a renamed facade:** not a goal. A forwarding facade could help a temporary migration, but it cannot be called the independent runtime and should not dictate its public API.
- **Rust bundler investigation or esbuild replacement:** explicitly deferred. Keep esbuild available in every current role unless a separate task changes that decision.
- **Unbounded backward compatibility for consumers:** not required. Change the owner's source and provide clear migration notes.

## 4. Compatibility, correctness, and migration

The following are different obligations:

| Obligation | Required? |
| --- | --- |
| Every Tailwind class/directive remains accepted | No. |
| Every Preact/React component continues to execute unchanged | No. |
| Existing third-party React packages run under the new runtime | No. |
| A selected supported utility has defined and deterministic output | Yes. |
| Authored CSS and required assets survive the chosen consumer migration | Yes. |
| Hydration preserves the DOM according to the documented contract | Yes. |
| Required user-visible behavior of the selected consumer is accounted for | Yes; retain it under the new model or explicitly record an intentional product change. |
| Old backend remains shipped forever | No. |

During implementation, the old engines may remain temporarily available to keep unrelated development moving or supply comparisons. Such a transition is bounded work, not a newly invented permanent support promise. Give temporary paths removal conditions and remove them after the relevant consumer and integration gates pass.

Breaking compatibility does not imply that missing styles, stale state, or accidental state loss are acceptable bugs. The owned specification becomes the behavioral authority. A deliberate change is documented and migrated; an unintended loss is fixed.

## 5. Design principles for agent development

1. **Few explicit rules.** Favor a small set of composable behaviors an agent can learn from local docs.
2. **Observable output.** Provide useful utility explanation and runtime mismatch diagnostics.
3. **Own the contract.** Upstream implementation details do not leak into the public API without a concrete reason.
4. **Reuse stable building blocks.** Owning a utility/runtime design does not require replacing V8, SWC, Lightning CSS, or esbuild.
5. **No imagined consumer scope.** Audit real downstream usage before selecting features or announcing completion.
6. **Behavior before optimization claims.** Establish correctness and measure a current baseline before claiming speed, size, or memory benefits.
7. **Independent engines.** CSS class generation must work for any markup producer. JSX rendering must work with ordinary authored CSS.

## 6. Evidence boundary

The source inspection used zfb commit `95d06aea3bc5b70b7f7df3253649673fef4da35a`. Its current CSS engine is subprocess-based; its native placeholder is not implemented. The rendering adapters and island runtime provide useful boundaries, but JSX assumptions also exist outside the adapter. The handoff has not performed an exhaustive downstream utility/hook audit or run new engines. See [sources](07-sources.md) and [integration](04-zfb-integration.md).
