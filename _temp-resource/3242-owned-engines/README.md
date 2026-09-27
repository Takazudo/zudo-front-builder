# Owned Engines — implementer resources

Temporary resources for the Owned Engines epic. This directory is deleted before the root PR merges.

## Authority order

1. The owner direction quoted in the epic issue.
2. The owned specs once they exist: `research/3242-zudo-wind-v1-spec.md`, `research/3242-zudo-react-v1-contract.md`.
3. The epic and sub-issue bodies.
4. `handoff/` — the original design handoff. It was written against an older commit, without compiling or running
   anything, and before the owner ordered aggressive removal. Where it says to keep old engines or defer features,
   the epic wins.
5. `exploration/` — read-only maps of this repository and of downstream consumers, measured on 2026-09-28 at
   `16bd41a8`. They are evidence, not instructions: line numbers drift, so re-open every file before editing it.

## Contents

| Path | What it is |
| --- | --- |
| `handoff/README.md` | Overview of the design handoff |
| `handoff/docs/01-decisions.md` … `07-sources.md` | Decisions, both engine specs, integration notes, delivery plan, verification, sources |
| `handoff/resources/*.json` | Machine-readable decisions and the 19 acceptance cases |
| `exploration/explore-*.md` | Nine subsystem maps: CSS engine, toolchain and embedding, renderer, islands, config and templates, in-repo consumers, tests and CI, dev orchestration |
| `exploration/census-*.md` | Removal censuses for Tailwind and Preact/React, docs and gate coupling, zudo-doc requirements |
| `exploration/consumer-requirements-summary.md` | Curated summary of what downstream consumers need |

Paths inside the maps are relative to the repository root unless they start with `$HOME/repos/`.
