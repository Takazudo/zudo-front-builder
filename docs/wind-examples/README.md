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
- Positive fragment-link examples may set `head: '<base href="about:srcdoc">'`.
  This exact value keeps links such as `href="#target"` in the preview document
  instead of inheriting the host page's base URL. The installed HtmlPreview shows
  this metadata in its native Head source panel. Other head markup is rejected;
  examples cannot introduce scripts, stylesheets or arbitrary document metadata.
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
It also hashes optional preview head metadata, so changing a fragment base requires
regeneration even when the utility CSS remains identical.
No binary path, temporary directory, timestamp, or machine-specific binary hash
enters the assets. Freshness compares exact bytes and the complete inventory,
including removed examples. A stale artifact fails without rewriting it.

Only positive samples produce iframe assets; diagnostic fixtures retain their
source and diagnostic contract in records and the manifest. The UI should show
those fixtures as diagnostic teaching examples, never as runnable previews.

`exampleAssetPath(family, id)` returns a base-neutral asset path;
`assetUrl(path, base)` prepends a configured URL base such as `/project/`.
For installed `HtmlPreview`, pass the CSS bytes and `exampleSource(example)` into its
isolated iframe rather than adding preview CSS to the parent document. The utility
and guide generators enforce the same HTML/CSS bytes and source hashes shown by
the built docs browser suite.

## Coverage contract

CI requires all 47 utility families and 187 catalog entries exactly once, with a
bilingual editorial record for every family. It also checks all ten English and
Japanese guide pages, all 57 source records, and the complete 168-example
inventory: 138 runnable positives and 30 expected diagnostics. Every utility and
guide example appears in both locales; the five `diagnostics.json` seeds are
pipeline-only fixtures and intentionally have no locale page. Positive previews
must match their committed HTML/CSS assets and manifest hashes. Expected
diagnostics stay textual and never become iframes.

Normal docs build/deploy consumes committed assets and never builds Rust.
The narrowly gated `wind-previews` job in `docs-checks.yml` builds one no-V8
workspace binary, checks all fixtures and freshness, and feeds the required,
fail-closed `Docs gate`. Compiler/catalog, records/config, generator scripts,
assets, and Rust manifests trigger it. The docs job checks both utility locales,
guide blocks, exact coverage, built routes and fragments, and native Chromium
previews in English and Japanese at root and genuine prefix mounts. Failure,
cancellation, detector failure, and unexplained skipping all fail the gate.
Content branches regenerate their assets before merge.

## Authored guide previews

Guides retain their authored prose. Mark a generated preview block with a stable
record reference and localized title:

```mdx
{/* wind-preview: guide-variants/responsive "Resize the preview" */}
{/* wind-preview:end */}
```

After the manager commits genuine assets, run
`node docs/scripts/generate-wind-guide-previews.mjs` with the owned guide paths.
Without paths it updates all ten guides in both locales; `--check` rejects stale
blocks. The generated native HtmlPreview receives the verified record HTML and
CSS, localized controls, and optional fragment base. It also shows the merged
configuration and authored scaffold in disclosures. Diagnostic records render
source and expected codes instead of an iframe. Do not hand-edit generated blocks
or duplicate their HTML/CSS elsewhere. Use `guide-` family names for guide records
to keep their identities distinct from utility families.

Operational rejection lessons have two diagnostic-only fields. Neither is allowed
on a positive preview, and neither changes the normal candidate-intent contract:

- `diagnosticStylesheet`: a separate CSS entry containing a rejected Tailwind
  directive, with expected `ZW009` / `error`. Keep `scaffoldCss` empty. A single
  quoted `@import "tailwindcss"` or `tailwindcss/...` import is also accepted as
  a rejection lesson; ordinary imports remain rejected. The guide generator displays these exact CSS bytes as source,
  never an iframe. This field does not allow Tailwind directives in scaffolding.
- `sourceExclusion: "all" | "partial"`: writes both `sample.html` and
  `excluded.html` from the same record HTML, and configures
  `wind.sources.exclude: ["excluded.html"]`. The `all` case explicitly reads
  `excluded.html` and expects `ZW010` / `error`; `partial` reads `*.html` and
  expects `ZW010` / `warning`. The generated lesson shows the two filenames,
  actual source glob and merged config. Arbitrary source-plan overrides remain
  rejected, and these records produce no preview assets.

`exampleInput`, `exampleSources`, and `exampleCompilerConfig` supply the same
stylesheet, source declaration, and configuration to compilation and guide source
rendering. Source-exclusion mode is also recorded in the freshness manifest.
