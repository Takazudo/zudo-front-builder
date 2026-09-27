# 06 — Verification and acceptance

## Purpose

This document defines meaningful evidence for the new engines. It is not a runnable test suite. The local agent must adapt cases to the chosen specification and the current repository's test strategy. `resources/acceptance-cases.json` contains the same core cases with stable IDs for task tracking.

There is no requirement for general Tailwind or React/Preact parity. There is a requirement for the behavior the owned specs promise. Preserve the repository's existing verification gates; change obsolete compatibility assertions deliberately, replacing them with assertions for the owned contract.

## Evidence levels

| Evidence | Suitable for |
| --- | --- |
| Pure compiler/logic | Parsing, token lookup, canonical ordering, immutable inputs, candidate refcounts, diagnostics. |
| Emitted artifact execution | Real generated JS modules, SSR HTML, CSS/resource publication, maps, installed binary behavior. |
| Real browser | Hydration identity, events, state updates, focus, input, cancellation, disposal, navigation. |
| Browser computed styles and visual review | Cascade, reset, media/state variants, layout, typography, migrated UI appearance. |

Use the least expensive level that can observe the relevant failure. A CSS string snapshot cannot prove the browser cascade. A source snippet containing `hydrate()` cannot prove that hydration ran. A screenshot cannot prove retained DOM identity or absence of subscription leaks.

## Wind cases

### W-A01 — Deterministic supported output

Compile the same explicit candidates/configuration with varied source traversal and candidate insertion order. Published CSS bytes and rule explanation results must be stable. Verify malformed recognized utilities in explicit inputs return structured diagnostics with the candidate and origin.

### W-A02 — Padding conflict behavior

Using spacing unit `0.25rem`, the proposed v1 fixture `p-4 px-2 pl-1` must compute top/bottom `1rem`, right `0.5rem`, and left `0.25rem`, across all class-order permutations. Test actual computed styles. If the local agent adopts different documented units/property policy, update the fixture values explicitly rather than calling accidental output compatible.

### W-A03 — States, breakpoints, and layers

Exercise a configured breakpoint at either side of its threshold, hover-capability handling, focus-visible, and root/descendant dark-mode matching. Verify authored component-layer rules, generated utilities, and unlayered authored overrides follow the specified cascade. Include a state combination that is rejected by the bounded grammar and ensure the diagnostic is useful.

### W-A04 — Source deletion and duplicate ownership

Put one candidate in two distinct sources. Remove it from the first: it stays. Remove it from the second: it disappears unless explicitly safelisted. Replace/remove a generated manifest and repeat. A renamed/deleted file and a newly added file under a configured glob must update the live candidate set. A fresh build and the warmed build must agree.

### W-A05 — Hidden client state and authored classes

Include a full class literal used only after a client interaction; it must be present despite absence from initial SSR HTML. Keep ordinary custom classes valid. Exercise a documented authored-class collision escape when needed. An unresolved dynamically constructed class must never be reported as successfully supported merely because a similar static fixture works.

### W-A06 — CSS assets and imports

Use a project stylesheet and a package stylesheet with distinct relative fonts/images, including a nested import. Verify URLs resolve against the declaring stylesheet, companion assets are emitted, content/base paths work, and browser requests succeed. Include an external/data URL following current asset policy. Minification and hashing must not lose provenance or alter required cascade behavior.

### W-A07 — Reset and ordinary CSS

Verify the exact reset fixture and `reset: none`. Include native controls, visible keyboard focus, headings/lists, and a border utility. The migrated page must not depend on removed Tailwind preflight rules accidentally. CSS Modules and global authored CSS still reach the published stylesheet.

### W-A08 — Installed distribution and process boundary

Build the chosen consumer with the installed/built zfb artifact using the new engine. Establish through appropriate process observation or a deliberately unavailable legacy executable that the new path does not invoke Tailwind. Verify its assets and live page. esbuild must continue to work in the relevant bundling paths. Do not use a mock engine to satisfy this case.

## JSX/runtime cases

### R-A01 — Server determinism and isolation

Render the same component and props twice, then render two different instances/requests. HTML and boundaries are deterministic for the same inputs; signal state does not leak between instances/requests. Exercise escaping of text/attributes and supported SVG/boolean/ARIA behavior. No browser activation callback runs on the server.

### R-A02 — Real adoption

Load the emitted SSR HTML, retain references to important DOM nodes, then load/activate the emitted runtime bundle. Assert those nodes are the same objects afterward. Include adjacent dynamic text, an empty text value, a fragment boundary, and two independent islands. The test must fail if hydration is replaced with clearing/recreating the subtree.

### R-A03 — Reactive semantics

Verify signal/computed objects in JSX update live text/attributes and plain `.value` reads follow the documented snapshot rule. Show that state changes do not rerun the component setup function. Test reactive props passed explicitly, computed dependency changes, and the chosen batch/flush semantics. Do not use timers to hide scheduling uncertainty.

For the static tabs specimen, combine a boolean `hidden` binding, the explicit authored hiding rule, and a `flex`/`grid` display utility. Assert actual computed `display: none` while inactive and the intended layout after showing it. Repeat with ordinary authored display CSS so the runtime does not depend on zudo-wind. Updating the hidden property alone does not satisfy this behavior.

### R-A04 — Events, refs, and activation

Native events reach the expected target and update state. Refs are assigned before activation callbacks. Repeated activation through zfb's lifecycle does not duplicate listeners or mount the same island twice. Native callback async work is supported according to the defined cancellation/disposal ownership; this does not require Suspense or streaming.

### R-A05 — Forms before hydration

Type into an uncontrolled input before its delayed island hydrates: its live value and focus survive adoption. For explicit model-bound controls, verify the selected dirty-DOM-to-model reconciliation rule before subscriptions can overwrite the field, and verify later programmatic updates. Include checkbox/select policies if these are included in the supported first consumer. Unsupported form binding shapes must fail explicitly.

### R-A06 — Composition and focus

Exercise compositionstart/update/end and input behavior under the chosen model binding. Programmatic updates must follow the documented composition policy. Include a real Japanese IME manual pass on a supported user platform when automation cannot faithfully generate OS composition; synthetic event coverage alone is labeled as such. Verify focus and selection for the supported operations.

### R-A07 — Disposal and cancellation

Remove an activated widget with a live computed subscription, custom observer/timer cleanup, and event listeners. Verify cleanup exactly once and no further observable work. Repeated dispose is safe. Remove a deferred island before activation and settle its pending import/visibility trigger; it must not activate after removal. A failed adoption must not leave effects/listeners attached.

### R-A08 — Mismatch and version rejection

Pair the server HTML with a deliberately incompatible marker/protocol version or structural description. Verify the specified diagnostic before activation effects run. The original DOM is not silently replaced. Malformed initial props are handled consistently. Add a multiple-root case so one failure cannot corrupt a neighboring island.

### R-A09 — zfb navigation ownership

Navigate away from a disposable island and verify release of work. Navigate through a page swap that intentionally persists an island and verify node/state identity and no duplicate mount. Exercise the current changed-props remount rule intentionally. The bridge must use actual zfb navigation and emitted island modules.

### R-A10 — Structural regions when required

For any implemented conditional region, verify proper activation/disposal on toggles and unambiguous SSR markers. For any implemented keyed list, reorder items while an input is focused and verify the intended item keeps its DOM/state/focus; remove one item and verify only its scope is disposed. Duplicate/invalid keys produce the specified diagnostic. If these features are absent, record them as unsupported and do not claim consumers requiring them have migrated.

### R-A11 — Runtime identity and generated integration

Build both current shared and per-island output modes when retained. Verify JSX factories, signal branding, server renderer, and browser hydrator resolve to the intended package identity and supported protocol. Inspect behavior from real built output so correct template strings with incorrect imports cannot pass. A nested component and a workspace-linked consumer should be included if they are supported rollout shapes.

## Combined migration and performance evidence

Maintain a consumer acceptance table listing each actual page/widget, relevant cases, intentional behavior changes, and remaining gaps. A styling/runtime specimen proves only its own scope. Finish chosen real consumer interactions and installed distribution before closing the migration.

Measure release builds of the current baseline and proposed implementation on the same platform/project. Record revisions, hardware/runtime context, cold versus warm setup, edit kinds, representative repeated samples, CSS/client output sizes, and relevant memory observations. Reuse existing timing output where it observes the phase. Do not treat esbuild, source staging, V8 reboot, and utility generation as a single unexplained number.

No performance target, byte ceiling, or test count is pre-approved here. Set a target only after measuring. Once focused risks and required gates are covered, stop broadening tests merely to accumulate passing counts.

## Completion report format

- **Implemented:** concrete engine/API/consumer changes.
- **Verified:** case IDs and relevant evidence, with current revisions/environment.
- **Intentional changes:** behavior that differs from previous Tailwind/Preact use.
- **Unsupported:** features deliberately outside the owned contract.
- **Unfinished/unverified:** actual remaining work or unavailable evidence.
- **Removed:** legacy paths deleted after their migration conditions passed.

Compatibility removal is a design decision. Clear evidence establishes whether the new design works.
