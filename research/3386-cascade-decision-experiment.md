# #3386 synthetic cascade decision experiment

This is an experiment-only comparison, not an approved product choice, compiler
emission test, installed Tailwind comparison, or consumer migration. No compiler,
configuration, API, CI gate, or existing placement assertion changes are included.
The existing `tests/wind-computed-style/w-placement.chromium.spec.mjs` already
covers actual current placement behavior; repeating that alone is not new evidence.

## Plan and verification ownership

Inherits `CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.
Integration parent: captured main `cb4b6dbd3af21980b302916c5d97d4f767c51574`.
No dependency on the other three sweep units; files are isolated. The child owns
fixture implementation, cheap syntax checks, foreground self-review and local
commit. The sweep manager owns serialized guarded Chromium execution, independent
review, integration and exact-head CI. No child browser/build runs, external writes,
release or deployment. This standalone `@verification` suite is intentionally not
added to a required gate; any promotion requires an explicit decision.

## Independent axes

The same 65 subjects run in twelve documents: three CSS models, two literal source
orders, and two declared layer orders. Border-top color is measured because it is
not inherited, so a hovered ancestor cannot fake a utility winner through inheritance.
Red is authored, green utility, blue the negative-control baseline. The baseline is
in `zw-reset`, rather than unlayered, so it cannot accidentally defeat layered
normal declarations when a relation is inactive.

| Model | Group selector | Peer selector | Utility location |
| --- | --- | --- | --- |
| current | `:where(.group:hover) .u` | `:where(.peer:focus) ~ .u` | unlayered |
| stronger-relations | `.u:is(:where(.group):hover *)` | `.u:is(:where(.peer):focus ~ *)` | unlayered |
| utility-layer | same relation shapes as current | same relation shapes as current | `@layer utilities` |

Current relation specificity is (0,1,0); stronger relation specificity is (0,2,0).
The stronger group shape is the exact shape documented in
`docs/src/content/docs/zudo-wind/coming-from-tailwind.mdx`, “Relation variant
specificity”, with a synthetic subject class replacing the escaped utility class.
The peer shape is the corresponding synthetic focus-state analogue; this does not
claim to capture a Tailwind package's bytes. `:where(.group)` / `:where(.peer)`
still contributes zero; the state is outside `:where()`. Neither relation becomes
self-matching, and peer remains a following-sibling relationship. Group hover keeps
the current `(hover: hover)` guard. Plain/media/dark specificity stays (0,1,0)
in every model: only relation specificity changes in the stronger model.

Literal stylesheet stages are `authored-before-utility` and
`authored-after-utility`; these names describe text order rather than reuse the
compiler's opposite-side placement labels. Each starts with one full prelude:

```css
@layer zw-reset, zw-tokens, zfb-hi, base, components, utilities;
/* or */
@layer zw-reset, zw-tokens, zfb-hi, base, utilities, components;
```

This orders layers before any rules regardless of stage order. Both orders remain
present for the unlayered models too, controlling that variable. The named layer
model deliberately retains current relation specificity, isolating layer precedence
from the stronger-selector experiment.

## Expected winners, not measured results

Each plain, group-hover, peer-focus, media and dark subject includes thirteen
scenarios. The oracle records these expectations independently of CSS parsing:

| Scenario | Current unlayered | Stronger relation, active | Named utility layer |
| --- | --- | --- | --- |
| Authored (0,1,0), normal | later rule | utility | unlayered authored |
| Authored (0,1,1), normal | authored | utility | unlayered authored |
| Authored (0,2,0), normal | authored | later rule | unlayered authored |
| Authored ID, normal | authored | authored | unlayered authored |
| Authored base/components, normal | utility | utility | later **declared layer** |
| Authored important, utility normal | authored | authored | authored |
| Utility important, authored normal | utility | utility | utility |
| Both important, authored unlayered (0,1,0) | later rule | utility | utility |
| Both important, authored base/components | authored | authored | earlier **declared layer** |

The stronger column applies only to group/peer; plain/media/dark use the current
column. With utilities last, important base and components beat important utilities.
With utilities middle, important base wins and important components loses. Normal
components reverses that result. Layered important declarations beat unlayered
important declarations; normal unlayered declarations beat normal layered ones.
Synthetic utility-important cases probe cascade mechanics, **not** an accepted Wind
important syntax or proposed important API.

Inactive group/peer/media/dark utility declarations leave every subject authored
red, including utility-important cases, because the utility does not match. Group
self, focused peer self, a previous sibling and a nested descendant stay baseline
blue. Focus/blur, hover enter/leave, dark ancestor/self enable/remove, viewport widths 639/640/800, and hover capability
are checked explicitly. Source order can settle stronger (0,2,0) authored ties;
stronger relations do not universally override authored CSS.

## Reproduction and evidence

From the repository root, with the repository's Playwright dependency and Chromium
installed, the manager runs:

```sh
CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium bash "$HOME/.codex/scripts/heavy-guard.sh" -- pnpm exec playwright test --config tests/cascade-decision-experiment/playwright.config.mjs
```

Omit `CHROMIUM_EXECUTABLE_PATH` to use Playwright's bundled Chromium; the optional
override uses the same Chromium engine and assertions, recording the chosen executable.
If repository dependencies are unavailable, install the frozen workspace dependencies
using the repository's normal setup, or point the manager's existing isolated
Playwright installation at this config. No Rust compilation, fixture server, SDK,
Tailwind package, or site build is required: Playwright loads literal HTML with
`page.setContent()`.

Artifacts live under `test-results/cascade-decision-experiment/`: every document
attaches its HTML, exact CSS, a JSON record of axes, selectors, expected colors,
measured colors by phase, browser version, executable path, platform and diagnostics, plus a final
screenshot and failure trace. Measurements are attached even when a winner fails;
assertions have zero retries and failures are not converted into expected failures.
Use reporter output for pass/fail status; JSON measurements alone are not a pass.

Child validation: `node --check` passed for `fixture.mjs`,
`cascade.chromium.spec.mjs`, and `playwright.config.mjs`; all twelve synthetic
fixtures were generated and inspected without a browser. Chromium execution and
measured outcomes remain manager-owned and pending at handoff. No native compiler
output, real consumer pages, WebKit/Firefox behavior, product adoption, or release
readiness is certified by these syntax and fixture checks. #3386 remains open for
owner interpretation of measured evidence and any separately approved product work.
