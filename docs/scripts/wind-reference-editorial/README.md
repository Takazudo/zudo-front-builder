# Authoring a utility family

Each content worker owns `<family>.json` here and the matching
`docs/wind-examples/<family>.json`. Keep English and Japanese in the same editorial
file. Generated utility MDX is output, never the authoring source.

The additive editorial schema is version 1:

- `family`: existing utility-family slug; filename and catalog membership must match.
- `lookup`: compact rows with `entry` (catalog ID belonging to this family),
  `candidate` (literal class), and `example` (positive example ID in the matching
  example record). Cover every catalog entry at least once. The candidate must be
  declared in that example's `utilities`. Exact-only entries must match the whole
  utility root after removing variants. Every row must emit all of its catalog
  entry's properties, preventing shared roots such as font from being assigned
  to the wrong semantic entry. Additional emitted properties are allowed.
  The generator reads the real emitted
  declarations from the committed compiler CSS, including child-selector rules;
  workers do not author a duplicate CSS/declaration string.
- `locales.en` and `locales.ja`: each has `purpose`, `setup`, `customValues`, and
  `examples`. Prose fields are plain text, escaped as MDX; write literal class names
  and configuration keys, without Markdown or raw JSX.
- Each localized `examples` item has `id`, `title`, and `description`. Both locales
  must reference the same examples in the same order and explain every lookup
  example. Put basic behavior first, then distinct axis/edge/selector behavior,
  then named/custom values where useful. A diagnostic example can appear here,
  but cannot supply a runnable lookup row.

The preview record contract is documented in
[the example README](../../wind-examples/README.md). Use static HTML, declare utility
and scaffold intent separately, and configure every required token, reset and
breakpoint. Scaffolding uses the `wind-demo` namespace and stays separate from
utility declarations. Choose `positive` or `expected-diagnostic` explicitly; retain
expected codes and severity for unsupported forms.

The generator uses `exampleSource(example)` unchanged for HtmlPreview and displayed
HTML. Positive previews receive inline committed genuine compiler CSS, with
`preflight={false}`, source open, and native source/copy/viewport controls. Inline
bytes avoid URL/base-path drift and keep the component's CSS source panel accurate.
Each example also exposes its merged compiler configuration (including strict
safelist and authored-class declarations) and its authored scaffold CSS. Expected
invalid samples show source and diagnostics without a runnable iframe.

After editing examples, notify the manager to regenerate assets serially before
running the page generator. Content workers do not invoke Rust, docs builds,
browsers or dev servers. The generator rejects missing assets or hashes that no
longer match HTML, CSS, scaffold input or configuration.

```sh
node docs/scripts/generate-wind-reference.mjs --locale en
node docs/scripts/generate-wind-reference.mjs --locale ja
node docs/scripts/generate-wind-reference.mjs --locale en --check
node docs/scripts/generate-wind-reference.mjs --locale ja --check
pnpm exec vp test run scripts/__tests__/generate-wind-reference.test.mjs
```

The generator loads each family independently in sorted filename order. During
rollout, `TRANSITIONAL_ALLOW_MISSING_EDITORIAL` explicitly preserves legacy output
for families without records. Final confirmation must set it to false; then a
missing family fails generation. All existing catalog h2/h3 headings remain
unchanged inside the optional technical disclosure, preserving public root,
duplicate and nested heading IDs. New editorial headings must not collide with
those IDs; the focused test checks both locales and every family.

The manager and dedicated confirmation tasks own strict site builds, emitted HTML,
freshness and real-browser checks: localized controls, source/copy accuracy,
viewport widths, computed utility behavior, keyboard access, narrow layouts,
themes, fragment links into technical disclosures and remount navigation.
