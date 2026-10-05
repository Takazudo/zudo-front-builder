# Genuine wind preview records

These locale-neutral records are the source of both displayed HTML and compiler
input. The editorial generator imports `loadRecords()` from
`docs/scripts/wind-preview-assets.mjs`; it must use `exampleSource(example)` for
source and preview, and may check its SHA-256 against the committed manifest.
This helper preserves the record HTML and adds the final source-file newline.
Never copy a second HTML string into generated MDX.

`types.d.ts` exports the discriminated `WindExample` union for generator consumers;
`loadRecords()` validates the same contract at runtime.

Each `<family>.json` has `schemaVersion: 1`, a stable `family` slug and an
`examples` array. Each example supplies:

- `id`: stable slug, unique in its family.
- `kind`: `positive` runnable preview, or `expected-diagnostic` teaching fixture.
- `html`: static literal HTML, with quoted class attributes.
- `utilities`: every utility-intended class, including markers. These are strict
  safelist inputs, so unknown ordinary names and foreign utilities fail.
- `authoredClasses`: every scaffold class, reserved in `wind.authoredClasses`.
  Use the `wind-demo` namespace; utilities and scaffolding must be disjoint.
- `scaffoldCss`: standalone authored demonstration CSS. Its class selectors must
  be declared scaffolding. It cannot import the host stylesheet or Tailwind.
- Optional `config`: recursive overrides to the shared compiler configuration.
  `null` removes a key, permitting missing-token teaching fixtures. Do not
  override candidate-source declarations. Only diagnostic fixtures may set
  `wind.strict: false` to teach source warning behavior.
- Diagnostic fixtures may set `candidateOrigin: "source"` or `"safelist"`
  (default). Source fixtures use literal HTML classes without an explicit
  safelist, preserving origin-sensitive severity; positive samples always use
  strict mode and an explicit safelist.
- Diagnostic fixtures require `expectedDiagnostics: [{code, severity}]` with
  `error` or `warning`. Fixtures with fatal diagnostics cannot mix severities.
  Positives allow no compiler diagnostics, including warning-only ZW014.

`base-config.json` explicitly selects spec 1, reset `none`, strict mode, no
breakpoints, and `tokens.spacingUnit: "0.25rem"`. Overrides allow scoped reset,
tokens, breakpoint, and theme demonstrations. No theme is inherited from docs.
The Gap and Padding seeds demonstrate numeric spacing with this unit.

## Regeneration and checks

The manager builds the workspace compiler once, or reuses a CI-built workspace
binary from the exact commit supplying the current compiler sources. For reuse,
verify that the compiler input closure has not changed, verify the downloaded
binary SHA-256, and record its successful workflow run, source commit, artifact
ID, and binary SHA-256 in the verification report. Then run from the repo root:

```sh
node docs/scripts/wind-preview-assets.mjs --compiler /absolute/path/to/workspace/zfb
node docs/scripts/wind-preview-assets.mjs --compiler /absolute/path/to/workspace/zfb --check
node --test docs/scripts/__tests__/wind-preview-assets.test.mjs
```

The compiler path is mandatory and absolute. `compiler.json` pins the expected
workspace CLI version (`0.0.0` for development), excluding docs' published
2.20.2 binary. After compiler changes, rebuild or obtain a new source-pinned CI binary before
regeneration. Reuse never bypasses the explicit binary path and version pin.
The command uses explicit input, output, config, project root, and HTML source,
with `--no-auto-source` and `--no-default-highlight-styles`. Utility intent is
also supplied as a strict safelist for positives and default diagnostic fixtures,
so compilation success cannot hide unknown
ordinary class names. Expected diagnostic codes and error/warning exit behavior
are checked during regeneration and freshness checking. Source ZW014 fixtures
contrast non-strict warning and strict error behavior.

Committed `docs/public/wind-examples/<family>/<id>.css` contains genuine compiler
output including declared scaffold CSS, with trailing whitespace normalized to
one final newline. Its sibling `.html` is the exact source
record plus a final newline. `manifest.json` records hashes of HTML, input CSS,
merged configuration, compiled CSS, and the workspace compiler source closure.
No binary path, temporary directory, timestamp, or machine-specific binary hash
enters the assets. Freshness compares exact bytes and the complete inventory,
including removed examples. A stale artifact fails without rewriting it.

Only positive samples produce iframe assets; diagnostic fixtures retain their
source and diagnostic contract in records and the manifest. The UI should show
those fixtures as diagnostic teaching examples, never as runnable previews.

`exampleAssetPath(family, id)` returns a base-neutral asset path;
`assetUrl(path, base)` prepends a configured URL base such as `/project/`.
For installed `HtmlPreview`, pass the CSS bytes and `exampleSource(example)` into its
isolated iframe rather than adding preview CSS to the parent document.
The forthcoming generator/UI owns that integration and source-hash enforcement.

Normal docs build/deploy consumes committed assets and never builds Rust.
The narrowly gated `wind-previews` job in `docs-checks.yml` builds one no-V8
workspace binary, checks all fixtures and freshness, and feeds the required,
fail-closed `Docs gate`. Compiler/catalog, records/config, generator scripts,
assets, and Rust manifests trigger it. Failure, cancellation, detector failure,
and unexplained skipping all fail the gate. Subsequent content branches must
regenerate their assets before merge.
