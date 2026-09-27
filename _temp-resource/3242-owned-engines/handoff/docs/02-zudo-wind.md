# zudo-wind: an owned utility CSS engine

## Status and authority

This is a proposed implementation specification for local development, prepared on 2026-09-28 JST. No zudo-wind engine has been implemented or benchmarked in this handoff. Repository observations refer to zfb snapshot `95d06aea3bc5b70b7f7df3253649673fef4da35a`; inspect the local checkout before editing.

**Owner decisions:** build an independently specified utility engine; Tailwind compatibility may be dropped; the owner controls adoption of future upstream ideas; esbuild remains. Embedding Tailwind in V8 is not a prerequisite. Migrate the owner's consumers directly instead of constructing a permanent compatibility subsystem.

**Recommended defaults:** native Rust implementation linked into zfb, a declarative rule catalog, explicit source ownership, and the grammar below. These are engineering recommendations, not previously approved details. A local agent may refine them when the consumer inventory supplies evidence, record the change, and proceed. Do not infer design-token policy from an uninspected zudo-css repository.

## 1. Product boundary

zudo-wind translates a finite vocabulary of complete class candidates and configured tokens into deterministic CSS. Its useful asset is a documented language with predictable behavior. It is not a promise to implement every Tailwind utility, directive, variant, reset rule, or plugin.

Include ordinary layout, spacing, sizing, typography, color, borders, a small state/breakpoint vocabulary, token references, and bounded arbitrary values. Add further families only when consumer requirements justify them. Initially omit JavaScript plugins, `@apply`, arbitrary selector variants, automatic safelist brace expansion, and compatibility with arbitrary third-party Tailwind component packages. Use authored CSS for unsupported expressions.

Tailwind v4.2.0 is the reference version pinned in the inspected zfb snapshot, not a claim about the latest upstream release. Selected reference outputs may help define adopted behavior; zudo-wind's own approved fixtures become authoritative. [zfb toolchain pins](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-toolchain-pins/src/lib.rs)

## 2. Local inventory before the first implementation

Inventory zfb's documentation and at least one real consumer, preferably zudo-doc or zzmod. Record complete classes, variants, arbitrary expressions, global CSS, imports, reset assumptions, custom properties, and Tailwind-specific directives. Include runtime branches, syntax-highlighting role classes, framework package sources, and generated template sources. Class frequency alone does not establish importance: a rarely used focus or error state still matters.

Produce a migration table with one disposition per relevant use: adopt unchanged, deliberately change, replace with authored CSS, or remove obsolete use. Preserve ordinary class names such as `site-header`; they are not automatically unsupported utilities. No corpus has been audited in this handoff, and no coverage percentage is claimed.

## 3. Configuration and tokens

Keep configuration data-only after zfb's existing configuration loader has evaluated its supported project file. The Rust engine receives serialized values; it does not execute JavaScript or discover plugins.

The following illustrates a proposed shape, not an existing public API:

```ts
wind: {
  spec: 1,
  reset: "minimal-v1",
  tokens: {
    spacingUnit: "0.25rem",
    colors: { panel: "var(--color-panel)", muted: "var(--color-muted)" },
    fontSizes: { sm: "0.875rem", base: "1rem" },
    radii: { card: "0.5rem" },
  },
  breakpoints: { md: { minWidthRem: 48 }, lg: { minWidthRem: 64 } },
  dark: { attribute: "data-theme", value: "dark" },
  safelist: ["hidden", "focus-visible:outline-solid"],
}
```

Source roots belong to a separate normalized `SourcePlan` supplied by zfb. Do not duplicate filesystem discovery in this configuration and in the engine.

Proposed token behavior:

- Token names use lowercase ASCII letters, digits, and internal hyphens; reject ambiguous duplicate bindings. Names such as `text-sm` must not simultaneously select a font-size token and a color token.
- Emit stable variables such as `--zw-color-panel` whose configured value can reference the project's own variables. `bg-panel` emits `background-color: var(--zw-color-panel)`. This introduces no project palette or theme policy.
- Emit all configured token variables initially. Avoid a dependency-pruning algorithm until there is evidence that token CSS is material to output size.
- Numeric spacing is nonnegative decimal multiples of `spacingUnit`. `p-4` means four units; `gap-1.5` means 1.5 units. Validate decimals and canonicalize without floating-point spelling drift. Negative values are legal only for catalog entries explicitly supporting them, such as margin.
- Breakpoints use a single sortable unit in v1. Positive finite `minWidthRem` values avoid ambiguous ordering of mixed units or runtime-dependent expressions.
- Validate each token against its expected CSS value category using existing parser facilities. References to external custom properties are allowed but do not prove that the project defines their runtime values.

Theme values are project configuration. Grammar, selector construction, declaration expansion, and conflict ordering are versioned engine behavior. Upstream theme variables illustrate the useful relationship between tokens and utilities, but do not define this schema. [Tailwind theme model](https://tailwindcss.com/docs/theme)

## 4. Candidate ownership and extraction

The `SourcePlan` is authoritative. zfb owns positive roots, exclusions, declared workspace/package sources, generated-source identities, explicit candidates, and extensions. Paths resolve relative to their declaring project/package, never whichever directory launched the process. Normalize aliases and symlinks for stable identity without broadening discovery into unrelated directories.

Exclude output directories, caches, dependencies not explicitly declared as sources, and binary files. Do not scan `dist` as the source of truth: client-only branches may not appear in initial HTML, and previous output can keep removed utilities alive.

Each scanned file produces a deduplicated set of candidates plus source locations. The extractor recognizes complete tokens from supported text sources; it does not evaluate expressions, execute source code, or guess values for `bg-${color}`. Literal maps of full class strings are supported. Safelists accept full candidates; generated templates can supply a finite manifest with a stable producer identity. A manifest's replacement removes its previous contribution.

Use a small tokenizer with delimiter awareness. A naive whitespace/colon split cannot parse bracketed values correctly. Track brackets, parentheses, quotes, and escapes; only split variant colons at top level. Test TSX, HTML, MDX, literal string maps, and escaped attribute text with actual extraction fixtures. Tailwind's complete-token detection is a reference for this deliberate limitation. [Tailwind source detection](https://tailwindcss.com/docs/detecting-classes-in-source-files)

The existing `crates/zfb-css/src/scanner.rs` discovers CSS Module imports; it is not an existing utility-class scanner. Keep those responsibilities distinct. Missing explicitly required inputs produce diagnostics; a filesystem deletion reported by the watcher removes a known source instead of silently retaining its previous classes. [Existing scanner](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/scanner.rs)

## 5. Proposed grammar and diagnostics

Use `variant:variant:utility` syntax and unprefixed utility names. Recognize fixed declarations (`flex`), numeric families (`p-4`), token families (`bg-panel`), and supported arbitrary-value families (`w-[37px]`). The catalog defines each family's accepted suffix and negative-value policy; recognize specific prefixes before general ones.

For v1, arbitrary values are a single property value enclosed by balanced brackets. Permit valid whitespace-free CSS such as `w-[var(--card-width)]` or `w-[clamp(12rem,40vw,30rem)]`. Do not implement Tailwind underscore-to-space rewriting or arbitrary property declarations initially. Expressions requiring spaces can be placed in a project token or ordinary CSS. Reject declaration separators, selector fragments, unsupported property/value combinations, and arbitrary `url()` values whose asset origin would otherwise be unclear. Validate with CSS parsing, not string substitution alone.

Return one of four explicit outcomes:

| Outcome | Build behavior |
|---|---|
| Recognized and valid | Emit rule; record its source and catalog entry. |
| Recognized utility with invalid value or unknown token | Report actionable diagnostic with candidate and location. |
| Ordinary or unrecognized class | Preserve authored HTML; emit no utility; list in audit output if requested. |
| Malformed or unsupported candidate in an explicit utility manifest/safelist | Fail with precise syntax/support diagnostic. |

Avoid warnings for every word in a prose file. Extraction confidence matters: enforce failures for explicit utility inputs and high-confidence class positions; classify ambiguous plain-text matches in audit output. Provide an explicit authored-class allowlist for custom names that collide with a recognized utility family; it suppresses generation and utility diagnostics for those exact names. Do not claim complete typo detection for dynamically constructed or unrecognized classes.

## 6. Variants and cascade are part of the language

Start with configured minimum-width breakpoints, `hover`, `focus`, `focus-visible`, `active`, `disabled`, and optional `dark`. Proposed hover behavior includes `@media (hover: hover)`. Dark mode uses the configured attribute on the document root; its selector also matches a root element carrying the utility. Wrap the root/ancestor test in `:where(...)` so that test does not add specificity.

Canonical order is breakpoint, then dark, then at most one state, then utility: `md:dark:hover:bg-panel`. Reject reordered, duplicate, or unsupported variants with a suggested canonical spelling. V1 does not need arbitrary combinations of several interaction states or parent/peer selectors. The restrictions simplify the language deliberately.

Emit a single documented layer declaration: `@layer zw-reset, zw-tokens, zw-components, zw-utilities;`. Place generated reset, tokens, and utilities in their layers; authored CSS may explicitly use `zw-components`. Unlayered authored CSS retains ordinary CSS precedence and can override layered utilities. Do not silently wrap imported third-party CSS to change its cascade.

Sort rules by a stable tuple: breakpoint rank (base first, then ascending widths), dark rank, state rank, property-conflict group, specificity within that property group, catalog rank, and normalized candidate bytes. Define state ranks explicitly in the catalog. This tuple determines generated order, not HTML class order. Normal CSS specificity still participates: document simultaneous state behavior rather than claiming tuple order overrides selector specificity.

Within the same variant bucket, broad padding precedes axis padding, which precedes side padding. Prefer explicit physical longhand declarations in v1: `p-4` sets all four sides; `px-2` sets left/right. The required fixture for `p-4 px-2 pl-1` yields top/bottom four units, right two units, and left one unit, in every class permutation. Repeat for margin and gap families; do not promise logical/physical property mixing until specified. For conflicting utilities at the same scope, use deterministic catalog/value ordering and report the conflict in audit output. The author should normally select one value.

## 7. Reset and ordinary CSS

Choose an explicit versioned reset, not an implicit copy of the current Tailwind preflight. Recommended `minimal-v1` contains only `border-box` sizing for elements/pseudo-elements, zero body margin, and inherited font on form controls. Record its exact CSS in a fixture. Do not silently remove outlines, list markers, heading styles, control appearance, or native hidden behavior. Allow `reset: "none"` for projects supplying their own reset.

Utilities must declare enough to work under that contract. For example, a border shorthand utility that promises a visible border must explicitly choose the needed style; it cannot depend on a removed upstream preflight rule. Inventory consumer assumptions and migrate their baseline stylesheet deliberately.

The minimal reset does not add a universal hiding rule. A consumer combining boolean `hidden` with a display utility must supply an explicit authored rule that preserves the intended visibility. The runtime's tabs specimen and R-A03 define such a rule and verify its computed display; this also works without zudo-wind. See [runtime specification](03-zudo-react.md) and [verification](06-verification.md).

Ordinary global CSS and CSS Modules remain supported. Remove Tailwind-only directives from migrated consumers; diagnostics should identify leftover `@theme`, `@source`, `@apply`, or imports of `tailwindcss` and point to the migration table. Do not silently ship these directives to browsers.

## 8. Integration, assets, and source maps

Implement a small engine crate/module and a zfb adapter at the existing `CssEngine` boundary. The inspected `NativeRustEngine` is a nonfunctional placeholder. The existing trait returns a CSS string and exposes package URL companions separately; its source paths are currently only hints. A new authoritative source-plan contract is therefore a deliberate API change, not a drop-in claim. [CSS engine contract](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/engine.rs), [Native placeholder](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/src/native_engine.rs)

Prefer a structured internal result containing CSS, source-map/provenance data, input dependencies, asset companions, and diagnostics. Adapt downstream consumers together; no compatibility requirement forces retaining a lossy string-only boundary. Keep each build result self-contained so a failed build cannot leak asset state from a previous call.

Retain zfb's CSS Modules processing, asset emission, hashing, and URL/base-path handling where they already work. Lightning CSS is already a direct Rust dependency; reuse it for supported parsing, transformation, and minification. Do not rewrite its CSS parser. [zfb CSS dependencies](https://github.com/Takazudo/zudo-front-builder/blob/95d06aea3bc5b70b7f7df3253649673fef4da35a/crates/zfb-css/Cargo.toml), [Lightning CSS Rust usage](https://lightningcss.dev/docs.html#from-rust)

Track each authored stylesheet's original identity before flattening imports. Resolve relative `url()` references against the declaring stylesheet, including package stylesheets. Record the import graph and asset dependencies; retain external/data URLs according to current asset policy. Compose transformation maps and use a stable virtual source such as `zudo-wind://spec/1` for generated rules, with separate candidate-to-source diagnostics. Never attribute generated CSS to an arbitrary first source file. Verify the minifier preserves effective layer/rule behavior.

## 9. Incremental builds and determinism

Maintain `source ID -> candidate set` and candidate reference counts across sources/manifests. A change replaces the previous set; deletion subtracts it. Emit a candidate while its count is positive, or while explicitly safelisted. The first implementation may re-emit sorted rules from the live set; correctness is more valuable than a complicated incremental serializer.

Cache keys include the spec/catalog version, normalized token/configuration values, source-plan identity, candidate sets, authored CSS/import contents, target browser settings, and asset dependency content. Exclude timestamps, absolute machine-specific paths in published CSS, traversal order, and temporary filenames. Glob expansion must notice new files. Import changes, token changes, and source-plan changes invalidate their dependents. In-memory warmed output and a clean rebuild must match byte for byte.

## 10. Catalog, explanation, stages, and acceptance

Each rule entry declares its identifier, grammar, declarations/emitter, supported values, token category, negative policy, conflict group, order, examples, and spec version. Shared metadata should generate reference docs and power proposed commands `zfb wind explain "md:hover:bg-panel"` and `zfb wind audit`. Explanations show token resolution, selectors, media conditions, declarations, ordering, and source use. These commands do not exist yet.

Implement in this sequence:

1. **Inventory and decisions:** commit the consumer migration table and approved v1 catalog/reset/cascade fixtures.
2. **Pure engine:** parser, token resolution, deterministic emission, and explanations from explicit candidate inputs.
3. **zfb integration:** authoritative source plan, authored CSS/imports/assets, CSS Modules, and correct incremental deletion.
4. **Consumer migration:** replace Tailwind directives and unsupported expressions; review actual pages and interactions.
5. **Distribution cleanup:** remove unused Tailwind extraction, warm-up, pins, and packaging paths after migrated builds pass. Keep esbuild integration unchanged.

Acceptance requires computed-style checks for spacing conflicts, breakpoint/state combinations, reset-dependent controls, and CSS layering; working imported fonts/images; client-only state classes present; no stale CSS after last-source removal; and byte-identical clean/warm outputs. Measure the same consumer's cold/warm build timings and CSS sizes before/after, without promising speedup. A bounded Tailwind comparison may assist selected fixtures during migration, but is not a permanent fallback or a requirement to match all upstream output.

If upstream source is copied, preserve the applicable copyright/license notices and record the source revision. Tailwind's MIT license includes that notice condition. [Tailwind v4.2.0 license](https://github.com/tailwindlabs/tailwindcss/blob/v4.2.0/LICENSE)
