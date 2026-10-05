# zudo-wind language specification v1

## Status and authority

Ratified for implementation under #3246, epic #3242. **Spec version: 1. Revision: 2.** Date: 2026-09-28. This is an owned contract, not a compatibility claim or evidence that the compiler already exists. Behavioral verification belongs to #3248–#3257 and #3269.

Authority, highest first: owner direction recorded in `research/3242-owned-engines-ledger.md`; this owned spec; epic and sub-issue bodies; the design handoff; exploration maps. A change to this document increments its revision and relocks affected implementation issues. A language-breaking change after release requires a new spec version.

Evidence labels below survive deletion of the temporary resource bundle:

- **L**: `research/3242-owned-engines-ledger.md`, “Basic-blog class candidates”, “Authored CSS, config files, and inline test configs”, and “Extractor-source examples”: 127 distinct scaffold tokens, 240 occurrences, 22 dark tokens, six child-combinator tokens, two arbitrary tracking values and five slash colours.
- **C**: ledger “Downstream evidence, quoted but not re-measured”, expanded at planning time by `_temp-resource/3242-owned-engines/exploration/census-zudo-doc-requirements.md`, sections utilityFamilies, variantsInUse, beyondV1Css: named spacing; group/peer states; five underscore-space arbitrary values; unlayered utilities; reset-dependent headings, lists and controls; missing tokens; 13 z-index tiers. These are dated aggregate requirements, not current measurements.
- **S**: planning-time `_temp-resource/3242-owned-engines/exploration/consumer-requirements-summary.md`: zudo-sg additionally needs named groups, attribute variants and pseudo-elements; bounded exclusions below are deliberate migration work for its owner.
- **P**: `crates/zfb-css/src/pipeline.rs`, `combine` and `splice_framework_after_layer_prefix`; `crates/zfb/src/commands/css_support.rs`, `role_classes_inline_sources`; `crates/zfb/src/config.rs`, `deep_merge`; ledger DD5, DD6, DD7 and DD18.

No private consumer identity or source inventory is reproduced. Every example below is an original specimen or a public in-repository scaffold token.

## Decision index

Exactly one disposition is assigned to each of the 29 requested features. “Adopt changed” includes a bounded supported subset; the restrictions in the linked contract are mandatory.

| ID | Feature | Disposition | Binding outcome and evidence |
| --- | --- | --- | --- |
| W01 | Grammar and tokenizer | adopt | Balanced delimiters; split separators only at top level. L, C. |
| W02 | Breakpoints, unit, max | adopt changed | Configured positive integer px widths; exclusive max range. C, P. |
| W03 | States | adopt | Eight states plus open; hover capability guard. L, C. |
| W04 | Group and peer | adopt changed | Unnamed markers and all W03 states; named forms rejected. L, C, S. |
| W05 | Pseudo-elements | adopt | before, after, marker, placeholder, backdrop; no implicit content. C, S. |
| W06 | Aria and data variants | reject with diagnostic | Attribute variants use authored CSS in v1. S; no scaffold requirement. |
| W07 | Arbitrary-selector variants | reject with diagnostic | Authored selectors replace them. C; none in L. |
| W08 | Stacking order | adopt changed | Responsive, dark, relation, state, pseudo-element; one of each. L dark:hover. |
| W09 | Dark | adopt changed | Optional configured attribute/value on self or ancestor, zero added specificity. L. |
| W10 | Colour opacity | adopt | Integer 0–100; color-mix in oklab against transparent. L, C. |
| W11 | Fractions | adopt changed | Positive integer ratios for sizing and translate only; sizing slash is never alpha. C. |
| W12 | Arbitrary values | adopt changed | CSS property validation; underscore becomes space; escaped underscore literal; no url. L, C. |
| W13 | Arbitrary properties | reject with diagnostic | Authored declarations instead. C, S. |
| W14 | Important modifier | reject with diagnostic | Leading and trailing important spellings rejected. C has zero uses. |
| W15 | Negatives | adopt | Per-family permissions, exact sign application. C. |
| W16 | Token schema | adopt changed | Data-only tokens, paired optional size leading, sizes and easings; DEFAULT maps to default. L, C. |
| W17 | Default tokens | adopt | No palette, spacing unit or named tokens shipped. L, C; scaffold supplies them. |
| W18 | Cascade and layers | adopt changed | Unlayered utilities; leading ordered layers and highlight placement; no final minifier. C, P. |
| W19 | Reset | adopt changed | none (default), minimal-v1, owned-v1; scaffold selects owned-v1. L, C. |
| W20 | Child-combinator utilities | adopt changed | space-x/y and divide-x/y use direct visible sibling pairs. L, C. |
| W21 | Families and batches | adopt changed | Closed catalog below; ring and animation move to authored CSS. L, C. |
| W22 | Diagnostic outcomes | adopt | Four outcomes, confidence-aware severity, structured origins. P, handoff section 5. |
| W23 | Authored-class allowlist | adopt | Exact full candidate suppression before validation or generation. L prose, C collisions. |
| W24 | Highlight role classes | adopt | Valid utility emits; every invalid or unknown role is ordinary. P. |
| W25 | Leftover directives | adopt changed | Hard errors on both enabled and authored-only paths, including imports. L, P. |
| W26 | Package manifests | adopt changed | Explicit versioned JSON, strict utility entries, producer-keyed maps survive merging. C, P. |
| W27 | Wind config | adopt changed | Strict nested objects, all optional fields defaulted; absent enables empty config after cutover. P DD5. |
| W28 | Classes emitting nothing | adopt changed | Missing token/default/breakpoint is an error at strict origins; no implicit theme repair. C. |
| W29 | Generated source identity | adopt | zudo-wind://spec/1 provenance; no generated source map in v1. P. |

## Grammar and rejection contract

Each G identifier is one independently tested production in #3248. EBNF braces mean repetition, brackets optional; quoted brackets are literal. Lexing happens before vocabulary or token lookup. Input is UTF-8; class candidates cannot contain literal whitespace, NUL or other control characters. CSS escapes inside bracket values are decoded for validation; class selector escaping always uses the original candidate bytes.

| ID | Production | Example |
| --- | --- | --- |
| G01 | candidate = { variant, ":" }, utility | sm:dark:hover:bg-panel |
| G02 | variant = breakpoint or max-breakpoint or dark or relation or state or pseudo | focus-visible:text-panel |
| G03 | breakpoint = configured-name; max-breakpoint = "max-", configured-name | max-sm:hidden |
| G04 | dark = "dark" (only when configured) | dark:block |
| G05 | state = hover or focus or focus-visible or active or disabled or first or last or focus-within or open | first:pt-0 |
| G06 | relation = ("group-" or "peer-"), state | group-open:rotate-90 |
| G07 | pseudo = before or after or marker or placeholder or backdrop | placeholder:text-panel |
| G08 | utility = ["-"], named, ["/", decimal] OR ["-"], root, "-[", value, "]", ["/", decimal] | -mt-2; bg-panel/40 |
| G09 | named = ASCII letters/digits/hyphens/dots beginning with a letter or digit; catalog splits longest root | gap-x-1.5 |
| G10 | decimal = digits, [".", digits]; digits are ASCII 0–9 | 0.5 |
| G11 | fraction = positive-integer, "/", positive-integer (suffix of an eligible root) | w-1/2 |
| G12 | value = nonempty balanced CSS component-value sequence, with no literal whitespace | grid-cols-[auto_1fr] |
| G13 | token-name = lowercase ASCII letters/digits separated by single hyphens, starting/ending alphanumeric | 2xl; hsp-sm |
| G14 | marker = "group" or "peer" (standalone, no variants/modifiers) | group |
| G15 | escape = backslash followed by a character inside value (CSS hex escapes follow CSS tokenization) | content-['a\_b'] is structurally tokenized; content is not a supported root |
| G16 | root = fixed root from the utility catalog, selected longest first | scroll-mt |
| G17 | positive-integer = digit 1–9 followed by zero or more digits 0–9 | 630 |
| G18 | configured-name = G13 name present in validated breakpoints | sm with minWidthPx 640 |

G08 is a structural representation: for G11 a top-level slash is stored until the catalog identifies sizing/fraction versus colour/alpha. Decimal inputs canonicalize for values, not class identities: p-01 and p-1 are distinct selectors with equal values. Numeric spellings are unsigned unless G08's leading negative is used. Parentheses and quotes outside brackets are illegal. A balanced value can contain nested parentheses/brackets, commas, colons, slashes, quoted strings, escapes and underscores. Colon and slash split only outside all brackets, parentheses and quotes. Literal whitespace must be written as underscore, including spaces in calc expressions. A trailing escape is malformed. Emit CSS identifiers using the CSS identifier escaping algorithm, including a hexadecimal escape plus terminator for a leading digit. Do not interpret an escaped colon in a class name as an alternative variant syntax.

Rejections are closed categories; each R identifier has an executable negative specimen. Parser tests may use a stub vocabulary and defer property validation to the resolver tests. After the allowlist/tolerant-origin policy, check malformed delimiters first, reserved unsupported syntax second (R07–R11), then variant vocabulary, variant order/cardinality, root, and value. This precedence makes `[&_a]:underline` R09/ZW004 rather than an unknown-variant diagnostic. Reserved unsupported syntax is recognized before the ordinary outer-character restriction too, so !block is R11, not R03.

| ID | Rejection | Example and outcome code |
| --- | --- | --- |
| R01 | Empty candidate/segment/body | hover::block → ZW001 |
| R02 | Unbalanced delimiter, unterminated quote or trailing escape | w-[calc(1px] → ZW001 |
| R03 | Literal whitespace/control or forbidden outer punctuation | w-(10px) → ZW001 |
| R04 | Unknown or unconfigured variant | md:block with no md → ZW002 |
| R05 | Duplicate variant class | hover:focus:block → ZW003 |
| R06 | Noncanonical variant order | hover:sm:block → ZW003, suggestion sm:hover:block |
| R07 | Named group or peer marker/variant | group/menu; group-hover/menu:block → ZW004 |
| R08 | Attribute variant | aria-pressed:block; data-[state=open]:block → ZW004 |
| R09 | Arbitrary-selector variant | [&_a]:underline → ZW004 |
| R10 | Arbitrary property | [overflow-wrap:anywhere] → ZW004 |
| R11 | Important prefix/suffix | !block; block! → ZW004 |
| R12 | Negative not permitted | -p-2 → ZW005 |
| R13 | Invalid fraction | w-1/0; w-0.5/2 → ZW005 |
| R14 | Invalid or misplaced opacity | bg-panel/101; p-2/50 → ZW005 |
| R15 | Empty, injected or invalid property value | w-[]; w-[red]; w-[1px;color:red] → ZW005 |
| R16 | URL function, including escaped/case-varied spelling, anywhere in token or arbitrary value | bg-[url(x)] → ZW005 |
| R17 | Unknown token or missing default/unit | rounded with no radii.default; p-4 with no spacingUnit → ZW006 |
| R18 | Invalid token name, ambiguous binding or reserved keyword | colors.center; radii.DEFAULT → ZW007 |
| R19 | Invalid config shape/value/version, duplicate breakpoint width | spec: 2; sm and md both 640 → ZW007 |
| R20 | Unsupported recognized utility family | ring-2; animate-spin → ZW004 |
| R21 | Unknown ordinary class in strict explicit input | safelist entry site-header → ZW008 |
| R22 | Variant or modifier on marker | hover:group → ZW004 |
| R23 | Child utility with a pseudo-element variant | before:divide-y → ZW005 |

R07–R11 are recognized before generic root lookup. Structural splitting for extraction preserves an invalid candidate in a high-confidence class position so R01–R03 can be diagnosed, even though it is not accepted as a candidate. Unsupported shapes inside low-confidence source literals are audit notes, not build errors. The authored allowlist and tolerant role origin take precedence over all R errors.

## Variants and canonical order

The canonical chain is **responsive → dark → relation → state → pseudo-element → utility**, with at most one member of each class. No min/max pair in one chain. State rank is none=0, first=1, last=2, open=3, focus-within=4, hover=5, focus=6, focus-visible=7, active=8, disabled=9. Relation ranks: none=0; group states in that order 1–9; peer states in that order 10–18. Pseudo ranks: none=0, before=1, after=2, marker=3, placeholder=4, backdrop=5. Missing/disabled dark is not an accepted variant. A pure ordering error returns the sorted spelling; duplicates and unknown variants have no automatic suggestion.

Breakpoints are positive integers in **px**, ordered by width; equal widths fail configuration. `sm:...` at 640 means `@media (min-width: 640px)`; `max-sm:...` means `@media (width < 640px)`. Do not subtract an epsilon. 640px, 1024px and 1280px correspond numerically to 40rem, 64rem and 80rem at 16px per rem; the contract uses px and does not change with root font size. Breakpoint names cannot be fixed variant names, start with max-, group- or peer-, or contain punctuation outside G13.

For escaped utility class selector C:

- first/last append `:first-child`/`:last-child`; open appends `:is([open], :popover-open)`; other states append their identically named pseudo-class.
- dark appends `:where([ATTRIBUTE="VALUE"], [ATTRIBUTE="VALUE"] *)`. The attribute may be on the element itself or any ancestor, including html; it is not restricted to html. Escape the configured quoted value as a CSS string.
- group-S uses `:where(.group:S) C`; peer-S uses `:where(.peer:S) ~ C`. Substitute the state selector above for S, including first/last/open. Group is an ancestor, peer a preceding sibling, never self. Each relation test contributes zero specificity.
- pseudo-elements append the named `::pseudo` last. before/after do not insert `content`; supply authored CSS when content is needed.
- any hover state, whether self, group or peer, adds `@media (hover: hover)`. Combine responsive and hover conditions with `and` in one media wrapper. A dark condition stays in the selector.

For example, `dark:hover:bg-panel` produces `.dark\:hover\:bg-panel:where([data-theme="dark"], [data-theme="dark"] *):hover` inside `@media (hover: hover)`. `group-hover:text-panel` uses `:where(.group:hover) .group-hover\:text-panel` under that same guard. These examples assume the corresponding configuration.

Child combinators are appended after the complete subject selector. A child utility with a pseudo-element variant is rejected as R23/ZW005 because it promises properties on children rather than that pseudo-element. Use authored CSS for such a combination. This is rejection R23, example `before:divide-y`.

## Tokens and configuration

The public zfb key and pure compiler object share the following data contract. All fields shown optional have the listed defaults. No implicit project palette, named scale, breakpoint, family, weight, radius, shadow or easing exists. `0` and `px` for spacing-like families and the closed catalog's static keywords are language constants, not theme tokens.

```ts
type Wind = false | WindConfig;
type StringMap = Record<string, string>;
type WindConfig = {
  spec?: 1; // 1
  reset?: "none" | "minimal-v1" | "owned-v1"; // none
  tokens?: { // every map defaults to {}
    spacingUnit?: string; // absent: nonzero numeric spacing is unavailable
    colors?: StringMap;
    spacing?: StringMap;
    sizes?: StringMap;
    fontSizes?: Record<string, { size: string; lineHeight?: string }>;
    fontFamilies?: StringMap;
    fontWeights?: StringMap;
    lineHeights?: StringMap;
    letterSpacings?: StringMap;
    radii?: StringMap;
    shadows?: StringMap;
    zIndices?: StringMap;
    easings?: StringMap;
  };
  breakpoints?: Record<string, { minWidthPx: number }>; // {}
  dark?: { attribute: string; value: string } | false; // false
  safelist?: Record<string, string[]>; // {}, owner id -> full candidates
  authoredClasses?: Record<string, true>; // {}
  manifests?: Record<string, { path: string }>; // {}, producer id -> manifest
};
```

JSON configurations:

```json
{}
```

```json
{ "wind": false }
```

```json
{
  "wind": {
    "spec": 1,
    "reset": "none",
    "tokens": {
      "spacingUnit": "0.25rem",
      "colors": { "panel": "var(--project-panel)" },
      "fontSizes": { "small": { "size": "0.875rem", "lineHeight": "1.25rem" } },
      "radii": { "default": "0.25rem" }
    },
    "breakpoints": { "sm": { "minWidthPx": 640 } },
    "dark": { "attribute": "data-theme", "value": "dark" },
    "safelist": { "app": ["sm:hover:bg-panel", "rounded"] },
    "authoredClasses": { "prose": true },
    "manifests": { "widgets": { "path": "@example/widgets/wind.json" } }
  }
}
```

After #3270 cutover, absent wind is exactly `{ "wind": {} }`: generation enabled, reset none, empty tokens/sources added only by zfb's source planner. A project with no stylesheet, no configured variables and no resolved rules emits no wind CSS and no stylesheet asset unless another pipeline part (CSS Modules/highlight) requires one. An explicitly selected reset emits even with zero candidates. `wind: false` bypasses generation, token validation, manifests and candidate scanning for wind in build/dev and standalone css; ordinary global CSS, Modules and highlight processing still run. Leftover-directive checks always run. The standalone `zfb css` command honors wind:false and emits authored CSS/highlight only; an absent key uses the empty enabled config even during the temporary seam. This deliberately overrides #3266’s recommendation to force generation when false. `zfb wind explain` with false returns a disabled outcome without generating CSS; audit reports generation disabled and an empty candidate report. The --source, --no-auto-source and highlight flags retain their discovery/highlight meanings when generation is enabled. No new CSS identity banner is added. False replaces an entire preset wind object. During the temporary #3264 seam only, absent wind still takes the pre-existing engine until #3270; the final absent-key semantics are mandatory after cutover.

Unknown keys in wind, tokens, each font-size object, breakpoint, dark object and manifest entry are errors; maps have arbitrary keys validated by their own rules. `null`, `true`, wrong field types and unsupported spec/reset values are errors. All optional objects default to empty objects, so partial preset fragments deserialize. For supplied dark objects both strings are required; attribute is a lowercase HTML attribute identifier `[a-z][a-z0-9-]*`, value is nonempty and contains no control characters. Every supplied breakpoint requires minWidthPx. Owner/producer ids are nonempty ASCII `[A-Za-z0-9][A-Za-z0-9._/-]*`; paths are nonempty. Authored class keys are nonempty whitespace/control-free complete class tokens, including punctuation; their values must be true.

Existing user-wins deep merge applies recursively to objects. Producer-keyed manifests and safelists merge distinct ids; the user's array replaces the preset array for the **same** safelist owner. Document this replacement; never concatenate implicitly. AuthoredClasses maps union with user-wins values (only true is valid). Set wind false to disable the complete system. There is no per-entry deletion sentinel in v1.

Token keys obey G13; uppercase DEFAULT is rejected and migrates to `default`. Bare rounded and shadow bind only radii.default and shadows.default respectively. Named spacing and sizes share a lookup namespace on sizing roots and must be disjoint. Colors/fontSizes share text; fontFamilies/fontWeights share font; each pair must be disjoint. Numeric-looking keys and reserved static suffixes for any consuming root are rejected (for example colors.center, radii.full, spacing.auto). A key may contain a numeric component such as neutral-50 or 2xl. `default` is allowed only in radii/shadows and also supports explicit rounded-default/shadow-default. There is no automatic case conversion.

Validate values with the existing Lightning CSS parser against their target property: colors/color; spacingUnit and spacing/padding (nonnegative length, percentage allowed only for spacing); sizes/width; fontSizes/font-size; paired leading and lineHeights/line-height; fontFamilies/font-family; fontWeights/font-weight; letterSpacings/letter-spacing; radii/border-radius (single nonnegative length-percentage only); shadows/box-shadow; zIndices/z-index (integer only); easings/transition-timing-function (single function/keyword). spacingUnit must be a single nonnegative dimension or zero, never percentage, calc or var. Numeric multiplication uses exact decimal digits, never floats; 1.5 × 0.25rem prints 0.375rem. Limits are 18 significant decimal digits for each input and result; overflow is ZW005 for a candidate or ZW007 for config. A nonzero scale with unit zero is valid and emits zero.

Every other category may reference an external `var()`; a syntactically valid, balanced, property-unparsed value with var is accepted as **category-unverified** and exposed in explain. It is not a reason to accept arbitrary garbage before/after var: parse an entire value, reject `!important`, semicolons, braces and URL functions recursively, and reject unparsed values without var. Do not promise to prove custom-property runtime types. Tokens may not reference the reserved `--zw-` namespace (cycles and engine composition collisions); reference project properties instead. Keywords inherit, initial, unset, revert and revert-layer are not token values. Do not perform theme namespace wipes or inline substitutions.

Variables are emitted sorted by category name then token name bytes in `@layer zw-tokens { :root { ... } }`, all configured values regardless of usage. Category prefixes: colors→color, spacing→spacing, sizes→size, fontSizes→font-size (paired leading → font-size-N-leading), fontFamilies→font-family, fontWeights→font-weight, lineHeights→leading, letterSpacings→tracking, radii→radius, shadows→shadow, zIndices→z, easings→ease. spacingUnit emits `--zw-spacing-unit`. Breakpoints and configuration are not CSS variables. A font-size key ending in `-leading` is forbidden (R18), preventing collision with paired leading variables.

Numeric spacing emits its exact computed dimension, named values emit var(--zw-PREFIX-NAME). Negative values wrap nonliteral values in `calc(V * -1)`; literal dimensions/numbers use one leading minus, with zero normalized to zero. CSS overrides can set project properties later: `--zw-color-accent: var(--color-accent)` resolves through the project property. For theme changes on nested elements, override `--zw-color-accent` on the same themed element as well because inherited custom property references resolve on their defining element; the scaffold toggles html only.

## Cascade, layers, reset and output

The exact generated leading statement is:

```css
@layer zw-reset, zw-tokens, zfb-hi, base, components;
```

Emit it whenever wind emits any reset, token or utility CSS; emit nothing for an entirely empty wind result. Append authored layer-order statements without pretending they can reorder these five names. Additional layer names acquire their ordinary CSS first-declaration order. Generated utilities are **unlayered**, after authored global rules; they beat normal declarations in all named layers. Unlayered authored rules of higher specificity still win; equal-specificity rules that truly occur after generated utilities win. Do not wrap imported stylesheets implicitly. Layered important author declarations follow CSS cascade rules; the language itself has no important modifier.

The integrated order is: layer prelude; hoisted external imports; framework highlight block `@layer zfb-hi`; generated reset and token layers; generated @property registrations; authored bundled CSS (including its layer statements and external-import placeholders removed by the bundler); unlayered generated rules; existing CSS Modules output at the pipeline's documented append position. #3264 must assert the resulting order with a populated highlight block and an external import. An existing authored layer prefix must not precede the generated prelude and change the owned layer ordering. Imports using layer(name) remain legal.

V1 generated output is **not minified**, in both dev and production. There is no newly introduced final-stylesheet minifier. Existing production CSS Module minification continues, so whole integrated stylesheets need not match dev and production bytes. Clean/warm byte identity is required within the same mode; pure wind CSS is identical between modes for identical inputs. No banner, timestamp or source-path comment is emitted. Fixed printer: LF, two spaces per indentation, one declaration per line, `property: value;`, selector/open brace on one line, closing brace on its own line, a single newline between rule blocks, one final LF. Empty result is zero bytes. CSS values use the validated serializer; declaration order is the catalog expansion order. The following reset blocks give exact unwrapped contents; embed bytes including final LF, indent once in the owning layer, and do not optimize or merge their rules.

`none` is the empty byte string:

```css
```

`minimal-v1`:

```css
*,
*::before,
*::after {
  box-sizing: border-box;
}
body {
  margin: 0;
}
button,
input,
optgroup,
select,
textarea {
  font: inherit;
}
```

`owned-v1` is this complete original reset (not minimal plus an unspecified extension):

```css
*,
*::before,
*::after {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
  border-width: 0;
  border-style: solid;
  border-color: currentColor;
}
html {
  line-height: 1.5;
  font-family: ui-sans-serif, system-ui, sans-serif;
  -webkit-text-size-adjust: 100%;
  tab-size: 4;
}
body {
  line-height: inherit;
}
h1,
h2,
h3,
h4,
h5,
h6 {
  font-size: inherit;
  font-weight: inherit;
}
a {
  color: inherit;
  text-decoration: inherit;
}
b,
strong {
  font-weight: bolder;
}
code,
kbd,
samp,
pre {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 1em;
}
ol,
ul,
menu {
  list-style: none;
}
button,
input,
optgroup,
select,
textarea {
  font: inherit;
  letter-spacing: inherit;
  color: inherit;
}
button,
input:where([type="button"], [type="reset"], [type="submit"]) {
  background-color: transparent;
  background-image: none;
}
textarea {
  resize: vertical;
}
img,
svg,
video,
canvas,
audio,
iframe,
embed,
object {
  display: block;
  vertical-align: middle;
}
img,
video {
  max-width: 100%;
  height: auto;
}
table {
  border-collapse: collapse;
  border-color: inherit;
  text-indent: 0;
}
summary {
  display: list-item;
}
hr {
  height: 0;
  border-top-width: 1px;
}
```

All resets leave native focus outlines and the appearance property untouched. No generated reset forces `[hidden]`; runtime fixtures must use their authored visibility rule even with display utilities. These are the entire reset guarantees. owned-v1 removes heading sizing, margins, list markers and link decoration, and inherits controls' font; it does not normalize every browser quirk. Bare border utilities independently establish a solid style under **all** reset choices.

The exact ascending rule sort tuple is:

```text
(responsive_rank, dark_rank, relation_rank, state_rank, pseudo_rank,
 conflict_group_rank, scope_rank, catalog_rank, raw_candidate_utf8_bytes)
```

responsive_rank: base=0; configured breakpoints ascending by width get min rank 2*i+1 and max rank 2*i+2 for zero-based i. Thus overlapping max variants have a defined order even when several match. dark rank is 0/1. Other variant ranks are above. Conflict group ranks are the catalog table order below, starting 1; family subentries have catalog rank in listed order, then root ASCII bytes when a cell groups roots. scope ranks: broad=0, axis=1, side/corner=2; a single-property family=0. This is emitted source order, not a specificity override. Candidates in the same bucket/group that write overlapping properties are audit conflicts; unrelated x/y longhands do not conflict. Variant classes and actual selector specificity still govern CSS.

For spacingUnit 0.25rem, every permutation of `p-4 px-2 pl-1` gives top=1rem, right=0.5rem, bottom=1rem, left=0.25rem. The corresponding margin triple gives the same values; `gap-4 gap-x-2 gap-y-1` gives column-gap=0.5rem and row-gap=0.25rem. Same-scope conflicts use final raw candidate byte order, not numeric value order: p-2 sorts after p-10. HTML class order has no effect.

## Utility catalog by family and batch

This is a closed vocabulary. Each row is a conflict group (its rank is row order); each listed root/static is a catalog entry with id `v1.<root>`; ambiguous roots use `v1.<root>.<kind>` (text.color/text.size/text.align, font.family/font.weight, border.width/border.color/border.style, outline.width/outline.color/outline.style, divide.width/divide.color). Longest root wins; exact statics precede token lookup, numeric/fraction parsing, then bracket value parsing. At shared roots, static keywords win, then disjoint token categories; arbitrary text uses property parsing to distinguish colour from length, while var without a resolvable category must use a named token. Ambiguous arbitrary values fail R15. This also applies to border/outline arbitrary colour versus width, and font arbitrary family versus weight. Parser-valid value is insufficient when the row forbids that value kind.

Notation: **S** = named spacing, exact numeric scale, 0, px or bracket length-percentage; **L** = nonnegative integer CSS pixels or bracket length (not spacing scale); **C** = configured colour, transparent, current, or bracket colour, optionally /0–100; **T(category)** = token or bracket value of that category. Named sizing first checks sizes then spacing. Every numeric integer range includes both ends. Negatives marked yes mean G08 unary negative; no other row accepts it. Physical longhands are mandatory for spacing, inset, border widths/colours and radius expansions. No logical-side utility aliases exist in v1.

| Rank/group | Batch | Entries, accepted suffixes and declarations | Tokens | Negative |
| --- | --- | --- | --- | --- |
| 1 display | A | block, inline, inline-block, flex, inline-flex, grid, inline-grid, hidden; display is spelling except hidden→none | none | no |
| 2 position | A | static, relative, absolute, fixed, sticky → position | none | no |
| 3 inset | A | inset, inset-x/y, top/right/bottom/left: S, auto, full→100%; expand corresponding offsets | spacing | yes except auto |
| 4 flex-container | A | flex-row/row-reverse/col/col-reverse → flex-direction; flex-wrap/wrap-reverse/nowrap → flex-wrap | none | no |
| 5 flex-item | A | flex-1→1 1 0%; flex-auto→1 1 auto; flex-initial→0 1 auto; flex-none→none; grow/shrink bare→1, -0→0, -[number] nonnegative → flex-grow/shrink | none | no |
| 6 grid | A | grid-cols/rows: 1–12→repeat(N,minmax(0,1fr)), none, subgrid, bracket track list; col/row-span: 1–12→span N / span N, full→1 / -1; col/row-start/end: 1–13, auto | none | no |
| 7 alignment | A | items/self: start,end,center,baseline,stretch (start/end→flex-start/end); justify: start,end,center,between,around,evenly (last three→space-*); place-items: start,end,center,stretch; properties align-items,align-self,justify-content,place-items | none | no |
| 8 sizing | A | size,w,h,min-w,max-w,min-h,max-h: S, size tokens, full, min/max/fit→min-content/max-content/fit-content, fractions; auto only size/w/h; none only max-w/max-h; screen→100vw or 100vh by axis, dvw/dvh→100dvw/100dvh; size sets width then height, forbids screen | sizes, spacing | no |
| 9 padding | A | p,px,py,pt,pr,pb,pl: S; broad then axes then sides | spacing | no |
| 10 margin | A | m,mx,my,mt,mr,mb,ml: S or auto | spacing | yes except auto |
| 11 gap | A | gap,gap-x,gap-y: S; column-gap then row-gap, x→column y→row | spacing | no |
| 12 child-space | A | space-x/y: S, set margin-left/top on matched later siblings | spacing | yes |
| 13 overflow | A | overflow,overflow-x/y: auto,hidden,clip,visible,scroll; overscroll,overscroll-x/y: auto,contain,none → overscroll-behavior | none | no |
| 14 z-index | A | z: integer 0–2147483647, auto, T(zIndices); bracket integer or var | zIndices | yes except auto |
| 15 font-family | B | font: T(fontFamilies) | fontFamilies | no |
| 16 font-weight | B | font: T(fontWeights); bracket weight | fontWeights | no |
| 17 font-size | B | text: fontSizes token emits size and optional paired line-height; bracket font-size emits size only | fontSizes | no |
| 18 line-height | B | leading: T(lineHeights), bracket length/number/percentage | lineHeights | no |
| 19 tracking | B | tracking: T(letterSpacings) | letterSpacings | yes |
| 20 text-layout | B | text-left/center/right/justify/start/end → text-align; whitespace-normal/nowrap/pre/pre-line/pre-wrap/break-spaces; break-normal→overflow-wrap:normal and word-break:normal; break-words→overflow-wrap:break-word; break-all→word-break:break-all; truncate→overflow:hidden,text-overflow:ellipsis,white-space:nowrap | none | no |
| 21 text-style | B | underline,overline,line-through,no-underline→text-decoration-line; uppercase,lowercase,capitalize,normal-case→text-transform; italic/not-italic→font-style; tabular-nums→font-variant-numeric:tabular-nums; antialiased→-webkit-font-smoothing:antialiased and -moz-osx-font-smoothing:grayscale | none | no |
| 22 color | B | text: C → color | colors | no |
| 23 background | B | bg: C → background-color (no images/gradients) | colors | no |
| 24 border-width | B | border,border-x/y/t/r/b/l: bare→1px or L; set corresponding width AND style:solid | none | no |
| 25 border-color | B | border,border-x/y/t/r/b/l: C; set corresponding colours | colors | no |
| 26 border-style | B | border-solid/dashed/dotted/double/none; border-collapse/separate→border-collapse | none | no |
| 27 radius | B | rounded,rounded-t/r/b/l/tl/tr/br/bl: bare→default token, T(radii), none→0, full→9999px; expand top-left,top-right,bottom-right,bottom-left in that order | radii | no |
| 28 divide-width | B | divide-x/y: bare→1px or L; later visible siblings get border-left/top-width and border-left/top-style:solid | none | no |
| 29 divide-color | B | divide: C; later visible siblings get border-color (four longhands) | colors | no |
| 30 outline-width | B | outline: L; width plus outline-style:solid; outline-offset: L | none | offset only |
| 31 outline-color | B | outline: C → outline-color | colors | no |
| 32 outline-style | B | outline-solid/dashed/dotted/double/none → outline-style; none changes style only | none | no |
| 33 shadow | B | shadow: bare→default token, T(shadows), none→none → box-shadow | shadows | no |
| 34 opacity | B | opacity: integer 0–100 divided by 100 exactly, bracket number 0–1 or percentage 0–100% or var | none | no |
| 35 transition | B | transition: bare or all→all; none→none; colors→color,background-color,border-color,text-decoration-color,fill,stroke; opacity→opacity; transform→transform,translate,rotate,scale; bracket property list | none | no |
| 36 duration | B | duration: integer 0–60000 milliseconds or bracket nonnegative time → transition-duration | none | no |
| 37 easing | B | ease: T(easings), bracket timing function → transition-timing-function | easings | no |
| 38 translate | B | translate-x/y: S or full or positive fraction, no size token; individual translate composition below | spacing | yes |
| 39 rotate | B | rotate: integer 0–360 degrees or bracket angle → rotate | none | yes |
| 40 interaction | B | cursor-auto/default/pointer/not-allowed/text/move/grab/grabbing/wait; pointer-events-auto/none; select-none/text/all/auto→user-select; resize bare→both, x→horizontal,y→vertical,none→none; accent: C→accent-color | colors for accent | no |
| 41 list | B | list-none/disc/decimal→list-style-type; list-inside/outside→list-style-position | none | no |
| 42 aspect | B | aspect-auto→auto,square→1 / 1,video→16 / 9, bracket ratio → aspect-ratio | none | no |
| 43 scroll-margin | B | scroll-m,scroll-mx/my,scroll-mt/mr/mb/ml: S → physical scroll-margin longhands | spacing | yes |
| 44 miscellaneous | B | align-top/middle/bottom/baseline→vertical-align; box-border/content→box-sizing; object-contain/cover/fill/none/scale-down→object-fit; sr-only exact expansion below | none | no |

The table's shorthand root lists are expansions, e.g. min-w and max-w are independent catalog entries. A recognized root matches only its exact spelling or its following hyphen suffix, not an arbitrary word prefix. Fixed-value aliases are explicit: flex-col→column, flex-col-reverse→column-reverse; no-underline→none, normal-case→none, not-italic→normal; box-border→border-box, box-content→content-box. Other listed keyword statics emit their documented suffix as the value. For p/m/inset/scroll-margin, broad expansions use top, right, bottom, left order; x uses left then right, y uses top then bottom. Border widths emit each edge's width then style in top/right/bottom/left order; border colours use the same edge order. Radius side groups have scope rank 1 and individual corners rank 2; size has rank 0 and width/height roots rank 1 in the sizing group. A missing suffix on any root without a stated bare form is R15/ZW005 (for example `p`). A numeric border or outline suffix uses pixels even when spacingUnit is absent. Width fractions accept positive integers up to 1000000 each; emit `calc(100% * N / D)` (no float rounding). Size fractions set both dimensions. Aspect ratio slash is inside brackets only; aspect-1/2 is R14. Built-in `full` and fractions are not opacity modifiers. Colour /N accepts only integers 0–100 and emits `color-mix(in oklab, VALUE N%, transparent)` without a supports wrapper. `current` means currentColor. /0 and /100 retain the same emitted function. Bracket colour may take /N; no other bracket family may.

Underscores in arbitrary values become spaces everywhere, including quoted strings; `\_` means literal underscore and the escape is removed. Other CSS escapes stay valid CSS escapes. `grid-cols-[auto_1fr]` resolves to grid-template-columns: auto 1fr. `tracking-[0.08em]` resolves to letter-spacing: 0.08em. `aspect-[1200/630]` resolves to aspect-ratio: 1200 / 630. Nested URL functions are forbidden even when escaped. Brackets cannot contain selectors, declarations or comments. `text-[var(--x)]` is ambiguous (colour or size); configure a named color/size token to disambiguate. An arbitrary value accepted as category-unverified is recorded in explain; there is no data-type hint grammar in v1.

Child utilities use subject selector `C > :not([hidden]) ~ :not([hidden])`; hidden-attribute siblings are excluded regardless of value, while display:none without the hidden attribute is not inspected. `space-y-2` is margin-top on each later eligible sibling, not a gap. `divide-y` is a top border on the same later siblings. No reverse flags; use authored CSS for reversed visual ordering. The child filter adds normal pseudo-class specificity; tests must measure authored-rule interactions.

All transition property entries except transition-none also set transition-duration:150ms and transition-timing-function:ease. transition-none emits only transition-property:none. Duration and easing groups sort after transition, so explicit values override those defaults within the same variant bucket. No upstream helper variables are used.

For translate composition only, emit these exact unlayered registrations once if a translate rule exists:

```css
@property --zw-translate-x {
  syntax: "<length-percentage>";
  inherits: false;
  initial-value: 0px;
}
@property --zw-translate-y {
  syntax: "<length-percentage>";
  inherits: false;
  initial-value: 0px;
}
```

translate-x/y sets its matching custom property then `translate: var(--zw-translate-x) var(--zw-translate-y)`. Noninheritance prevents a nested element inheriting its parent's offset. Rotate sets the independent rotate property; it composes with translate through CSS individual transforms. No scale utility, transform-string utility, animation or ring utility is supported. Ring and animate prefixes are recognized unsupported (R20); scale/transform prefixes are also recognized unsupported with R20, e.g. scale-95. Use authored CSS, or outline utilities when the intended appearance is an outline. `ease-in-out` is a named easing lookup and errors until easings.in-out is configured. No animation keyframes are synthesized.

sr-only expands, in order, to position:absolute; width:1px; height:1px; padding:0; margin:-1px; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border-width:0. Each catalog entry must carry at least one example with expected declarations; #3250/#3251 author and execute them from this table, and #3255 exports those same examples. Conflict detection uses actual property write sets, including incidental styles/default transition declarations and sr-only expansions, so overlap across nominal groups is visible to audit. Group ranks still control emission.

## Diagnostics, explain and audit

Outcomes: resolved utility (emit); recognized but invalid (diagnostic); ordinary unrecognized class or marker (no CSS); malformed/unsupported/unknown explicit utility input (fail). All diagnostic records have severity, code, candidate (nullable for config/file errors), origin, message, optional suggestion and optional rejectionId. Origin kinds: source (stable source id, UTF-8 byte offset/length, one-based line and byte-column, enclosing literal span, positionKind class/literal), safelist (owner and index), manifest (producer, path, index), config (key path), roleClass (role key), stylesheet (path/span). Retain all occurrences; report one diagnostic per code/candidate/origin and sort by origin id, offset or key/index, code, candidate bytes.

| Code | Meaning | Severity and build behavior |
| --- | --- | --- |
| ZW001 | Candidate syntax | error at class/safelist/strict manifest; audit-only info at other literals |
| ZW002 | Unknown/unconfigured variant | same |
| ZW003 | Variant cardinality/order | same; suggestion only for sortable unique chain |
| ZW004 | Deliberately unsupported language | same |
| ZW005 | Invalid value/modifier/negative | same |
| ZW006 | Missing token, unit or default | same |
| ZW007 | Config/token schema or value | error, pass fails |
| ZW008 | Unknown explicit utility | error for safelist/manifest; source ordinary classes are not errors |
| ZW009 | Leftover directive | error on both wind modes, pass fails |
| ZW010 | Required source/manifest missing, unreadable or invalid | error, pass fails |
| ZW011 | Scanned source not UTF-8 | warning, skip file and remove its old contribution |
| ZW012 | Dynamic candidate construction | audit-only info, no invented utility |
| ZW013 | Property conflict in one class literal | audit-only info, deterministic winner explained |

An unknown root such as site-header in a class attribute is ordinary. A recognized prefix such as text-missing is ZW006. Low-confidence complete literals still generate valid rules; invalid ones never break builds. `codeHighlight.roleClasses` is deliberately tolerant even for a recognized invalid shape: valid utilities emit, otherwise ordinary with no diagnostic (audit may label roleClass as ignored). Exact authoredClasses matches suppress validation, emission and utility diagnostics at **all** candidate origins; config schema and leftover stylesheet checks cannot be suppressed. Markers group/peer are valid explicit entries with metadata kind marker.

Compile returns diagnostics and metadata even on error; callers must not publish partial CSS on an error. Failed dev recompilation keeps the last successful served artifact but surfaces the error; it is not a successful fresh/warm result. Explain exposes parse, entry id, spec/revision, resolved tokens including category-unverified flag, declarations, selectors, conditions and full sort tuple. Audit includes ordinary class-position names (excluding markers/allowlist), missing/dead utilities, dynamic fragments and property conflicts within an enclosing class-list literal, not across unrelated elements.

## Sources, safelist and package manifests

The SourcePlan alone controls discovery: project conventional roots, declared package roots, mirror roots, explicit CLI sources, generated sources and manifests. Resolve paths relative to the declaring project/preset/package; retain that declaration base before config merging. Never resolve from the process working directory. File-backed paths in manifest entries resolve package exports if they are package specifiers, otherwise relative to the declaring configuration file. A merged manifest retains its declaring base; user replacement replaces the base too. Conflicting paths for the same producer after normalization are a config error, not two owners.

Build/dev source-plan extensions are exactly **.tsx, .ts, .jsx, .js, .mdx, .md** (case sensitive). **.html and .mjs are not automatically scanned** in build/dev v1, including declared directory roots; no #3263 classifier expansion is required. The extractor still supports all eight kinds for pure compiler tests and standalone `zfb css --source` inputs: the standalone explicit plan may include .html/.mjs in its extension set. There is no public wind.extensions option. Docs and fixtures must distinguish extractor capability from build/dev discovery. Explicit unsupported file extensions are ZW010, not silently ignored.

Never scan previous output. Exclude node_modules unless an explicit declared package root, dist, build, target, .git, .cache, .zfb-build, .zfb and configured output/cache directories, on path-component boundaries. The explicit package-root exception permits walking inside node_modules and a published package’s dist directory only for that declared root, not ambient dependencies. Basename skip rules apply below each root; an explicitly declared package-route entrypoint directory may itself lie inside node_modules/pkg/dist and remains traversable. The project’s configured output/cache directories and current pass output remain absolute exclusions even if declared explicitly; published package dist is not the consuming project’s generated output. Gitignore applies below each declared root; a root explicitly declared inside a parent-ignored directory remains traversable. Do not follow directory symlinks during recursive walking; canonicalize an explicitly supplied symlink root/file once, deduplicate canonical files, preserve the declared stable root label and relative path for diagnostics. Missing conventional roots skip; missing required roots and zero-match explicit CLI globs error. Source identity uses stable root label and relative forward-slash path, never an absolute machine path in published CSS.

Extract complete class/className strings in JSX/HTML; all script string/template literals including inline scripts; literal maps; nested literals within interpolation; raw HTML/JSX attributes in MD/MDX. Decode source-language escapes and HTML entities before candidate lexing while preserving original source spans. Ignore comments, Markdown prose, inline code and fenced code. A script string containing HTML is inspected for embedded attributes as class confidence. Literal class strings passed to runtime classList APIs remain supported through the general literal scan.

At an interpolation boundary, retain a static token only if it is independently complete with a suffix: `last:border-b-0${x}` retains last:border-b-0 with an adjacentInterpolation audit flag. `bg-${color}` and `${prefix}-500` are dynamic fragments, emit nothing; do not concatenate across interpolation. A syntactically complete fragment can still differ from the runtime joined class, so audit does not promise coverage. A finite literal map or safelist is the supported way to make state-dependent classes available.

Index operations replace each owner's full set; duplicate owners keep a candidate live until the last owner disappears. Safelist owner arrays and generated manifest producers are separate identity namespaces. Replacing/removing a manifest retracts its prior contribution. Watch additions, deletions, renames, manifest changes and config/source-plan changes. A required manifest removed from disk fails the pass; removing its declaration from config intentionally removes that owner. A clean build of final state and a successful warmed build of that state must have identical candidates and mode-specific CSS bytes. Do not keep removed candidates as a safety net.

Manifest schema version is independent of language spec version. This is the exact JSON shape (unknown keys rejected):

```ts
type WindCandidateManifestV1 = {
  schemaVersion: 1;
  specVersion: 1;
  producer: string;
  candidates: string[];
};
```

```json
{
  "schemaVersion": 1,
  "specVersion": 1,
  "producer": "widgets",
  "candidates": ["group", "p-0", "hover:bg-panel"]
}
```

The consumer declares `manifests.widgets.path`; producer must exactly equal its map key. No package.json auto-discovery, ambient node_modules walk, manifest-provided tokens or executable plugins. Producers publish only complete utility candidates/markers. Every non-utility entry errors unless the consumer exactly allowlists it. This deliberately requires filtering old broad package safelists. Duplicate candidate strings are deduplicated; invalid UTF-8/JSON/schema/missing file is ZW010; candidate errors retain their ZW001–ZW008 code and manifest index. File and generated in-memory manifests share producer replacement semantics; two simultaneously declared origins claiming a producer error rather than winning by traversal order. The configuration safelist uses owner-keyed arrays, has the same strict candidate rules, and supports no regex, brace expansion or interpolation.

## Leftover directives, integration and provenance

Reject Tailwind imports (`tailwindcss` and every `tailwindcss/` subpath), @tailwind, @theme (all modifiers), @source, @custom-variant, @apply, @utility, @variant, @plugin, @config and @reference. Code ZW009 names the spelling, originating stylesheet file/span, and migration page `/docs/zudo-wind/coming-from-tailwind/` (docs task must create or route this path). Scan authored entry and every imported stylesheet before import resolution attempts to resolve a forbidden import, including wind:false. Use token-aware scanning, ignoring comments and string contents; escaped CSS identifiers/import strings must be decoded. Ordinary @layer and `@import "package/style.css" layer(components)` remain valid. Legacy theme(), --spacing(), --alpha() and --value() functions in declaration values also produce ZW009; strings/comments containing their names do not.

Asset attribution remains the authored stylesheet's real identity. Relative package url() resolves against its declaring stylesheet; nested imports retain their own identity. Project/external/data URL behavior follows the existing asset contract owned by #3256. Wind arbitrary values cannot add assets. CSS Modules remain separately processed.

Every generated reset, token, registration and utility rule has virtual source **zudo-wind://spec/1**. V1 produces no generated source map and makes no line-mapping promise. The #3260 engine result provenance slot must carry a structured record with source id `zudo-wind://spec/1`, kind generated, specVersion 1, specRevision 2, and no map (adapt field spelling to its actual type); it must not be None for nonempty wind output. Candidate→all source locations lives separately in rule metadata/diagnostics. Empty wind output has no generated provenance entry; mixed authored/generated output preserves authored bundle provenance in addition to this generated record. Do not attribute wind rules to the first input file. Cache identity includes spec and revision, normalized config, target settings, source-plan identity, candidate sets, authored imports and asset contents; exclude traversal order, timestamps and absolute machine paths.

## Basic-blog migration

This is the exact row set from ledger L: **127 distinct class tokens**. Every token remains spelled as written. Marker group is adopted as a marker; prose is adopted as the existing authored class, entered in authoredClasses. Adoption of the 125 utility candidates is contingent on the explicit scaffold configuration below, not a compiler default theme. Authored .prose rules remain in components; first/hover/group/dark/opacity/child-selector semantics are those above. #3267 must re-extract and report new tokens rather than guessing their disposition.

| Original class token | Disposition | Result |
| --- | --- | --- |
| `antialiased` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-amber-50` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-emerald-50` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-neutral-100` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-rose-50` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-sky-50` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-violet-50` | adopt | Keep; resolve with scaffold tokens below. |
| `bg-white` | adopt | Keep; resolve with scaffold tokens below. |
| `block` | adopt | Keep; resolve with scaffold tokens below. |
| `border` | adopt | Keep; resolve with scaffold tokens below. |
| `border-amber-500` | adopt | Keep; resolve with scaffold tokens below. |
| `border-b` | adopt | Keep; resolve with scaffold tokens below. |
| `border-emerald-500` | adopt | Keep; resolve with scaffold tokens below. |
| `border-l-4` | adopt | Keep; resolve with scaffold tokens below. |
| `border-neutral-200` | adopt | Keep; resolve with scaffold tokens below. |
| `border-rose-500` | adopt | Keep; resolve with scaffold tokens below. |
| `border-sky-500` | adopt | Keep; resolve with scaffold tokens below. |
| `border-t` | adopt | Keep; resolve with scaffold tokens below. |
| `border-violet-500` | adopt | Keep; resolve with scaffold tokens below. |
| `cursor-pointer` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-amber-950/40` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-emerald-950/40` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-neutral-900` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-neutral-950` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-rose-950/40` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-sky-950/40` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:bg-violet-950/40` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:border-neutral-800` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:divide-neutral-800` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:hover:border-neutral-700` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:hover:text-neutral-100` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-amber-300` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-emerald-300` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-neutral-100` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-neutral-300` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-neutral-400` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-neutral-50` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-neutral-500` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-neutral-600` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-rose-300` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-sky-300` | adopt | Keep; resolve with scaffold tokens below. |
| `dark:text-violet-300` | adopt | Keep; resolve with scaffold tokens below. |
| `divide-neutral-200` | adopt | Keep; resolve with scaffold tokens below. |
| `divide-y` | adopt | Keep; resolve with scaffold tokens below. |
| `first:pt-0` | adopt | Keep; resolve with scaffold tokens below. |
| `flex` | adopt | Keep; resolve with scaffold tokens below. |
| `flex-col` | adopt | Keep; resolve with scaffold tokens below. |
| `flex-wrap` | adopt | Keep; resolve with scaffold tokens below. |
| `font-medium` | adopt | Keep; resolve with scaffold tokens below. |
| `font-mono` | adopt | Keep; resolve with scaffold tokens below. |
| `font-semibold` | adopt | Keep; resolve with scaffold tokens below. |
| `gap-2` | adopt | Keep; resolve with scaffold tokens below. |
| `gap-4` | adopt | Keep; resolve with scaffold tokens below. |
| `gap-5` | adopt | Keep; resolve with scaffold tokens below. |
| `gap-x-3` | adopt | Keep; resolve with scaffold tokens below. |
| `gap-y-2` | adopt | Keep; resolve with scaffold tokens below. |
| `group` | adopt | Keep marker; emits no CSS. |
| `group-hover:text-accent` | adopt | Keep; resolve with scaffold tokens below. |
| `grow` | adopt | Keep; resolve with scaffold tokens below. |
| `hover:border-neutral-300` | adopt | Keep; resolve with scaffold tokens below. |
| `hover:text-neutral-900` | adopt | Keep; resolve with scaffold tokens below. |
| `hover:underline` | adopt | Keep; resolve with scaffold tokens below. |
| `inline-block` | adopt | Keep; resolve with scaffold tokens below. |
| `items-center` | adopt | Keep; resolve with scaffold tokens below. |
| `justify-between` | adopt | Keep; resolve with scaffold tokens below. |
| `justify-center` | adopt | Keep; resolve with scaffold tokens below. |
| `leading-relaxed` | adopt | Keep; resolve with scaffold tokens below. |
| `max-w-2xl` | adopt | Keep; resolve with scaffold tokens below. |
| `min-h-screen` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-1` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-12` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-14` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-2` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-3` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-4` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-6` | adopt | Keep; resolve with scaffold tokens below. |
| `mt-8` | adopt | Keep; resolve with scaffold tokens below. |
| `mx-auto` | adopt | Keep; resolve with scaffold tokens below. |
| `my-6` | adopt | Keep; resolve with scaffold tokens below. |
| `pb-6` | adopt | Keep; resolve with scaffold tokens below. |
| `prose` | adopt | Keep existing .prose rules; authoredClasses.prose = true. |
| `px-2` | adopt | Keep; resolve with scaffold tokens below. |
| `px-4` | adopt | Keep; resolve with scaffold tokens below. |
| `px-5` | adopt | Keep; resolve with scaffold tokens below. |
| `py-0.5` | adopt | Keep; resolve with scaffold tokens below. |
| `py-12` | adopt | Keep; resolve with scaffold tokens below. |
| `py-16` | adopt | Keep; resolve with scaffold tokens below. |
| `py-2` | adopt | Keep; resolve with scaffold tokens below. |
| `py-3` | adopt | Keep; resolve with scaffold tokens below. |
| `py-5` | adopt | Keep; resolve with scaffold tokens below. |
| `py-6` | adopt | Keep; resolve with scaffold tokens below. |
| `rounded-full` | adopt | Keep; resolve with scaffold tokens below. |
| `rounded-md` | adopt | Keep; resolve with scaffold tokens below. |
| `scroll-mt-8` | adopt | Keep; resolve with scaffold tokens below. |
| `size-8` | adopt | Keep; resolve with scaffold tokens below. |
| `sm:flex` | adopt | Keep; resolve with scaffold tokens below. |
| `sm:gap-6` | adopt | Keep; resolve with scaffold tokens below. |
| `sm:mt-0` | adopt | Keep; resolve with scaffold tokens below. |
| `sm:px-6` | adopt | Keep; resolve with scaffold tokens below. |
| `sm:shrink-0` | adopt | Keep; resolve with scaffold tokens below. |
| `sm:w-48` | adopt | Keep; resolve with scaffold tokens below. |
| `space-y-2` | adopt | Keep; resolve with scaffold tokens below. |
| `space-y-3` | adopt | Keep; resolve with scaffold tokens below. |
| `space-y-4` | adopt | Keep; resolve with scaffold tokens below. |
| `tabular-nums` | adopt | Keep; resolve with scaffold tokens below. |
| `text-2xl` | adopt | Keep; resolve with scaffold tokens below. |
| `text-3xl` | adopt | Keep; resolve with scaffold tokens below. |
| `text-accent` | adopt | Keep; resolve with scaffold tokens below. |
| `text-amber-700` | adopt | Keep; resolve with scaffold tokens below. |
| `text-center` | adopt | Keep; resolve with scaffold tokens below. |
| `text-emerald-700` | adopt | Keep; resolve with scaffold tokens below. |
| `text-lg` | adopt | Keep; resolve with scaffold tokens below. |
| `text-neutral-400` | adopt | Keep; resolve with scaffold tokens below. |
| `text-neutral-500` | adopt | Keep; resolve with scaffold tokens below. |
| `text-neutral-600` | adopt | Keep; resolve with scaffold tokens below. |
| `text-neutral-700` | adopt | Keep; resolve with scaffold tokens below. |
| `text-neutral-900` | adopt | Keep; resolve with scaffold tokens below. |
| `text-rose-700` | adopt | Keep; resolve with scaffold tokens below. |
| `text-sky-700` | adopt | Keep; resolve with scaffold tokens below. |
| `text-sm` | adopt | Keep; resolve with scaffold tokens below. |
| `text-violet-700` | adopt | Keep; resolve with scaffold tokens below. |
| `text-xs` | adopt | Keep; resolve with scaffold tokens below. |
| `tracking-[0.08em]` | adopt | Keep; resolve with scaffold tokens below. |
| `tracking-[0.2em]` | adopt | Keep; resolve with scaffold tokens below. |
| `tracking-tight` | adopt | Keep; resolve with scaffold tokens below. |
| `transition-colors` | adopt | Keep; resolve with scaffold tokens below. |
| `uppercase` | adopt | Keep; resolve with scaffold tokens below. |

### Scaffold declarations and stylesheet edits

The following is the complete scaffold wind value. Hex values are the owned scaffold palette: they are explicit sRGB design choices, not aliases to an installed library. They intentionally approximate the prior appearance; #3267 must list any measured visual difference against #3245 rather than certify pixel identity. No other project receives them implicitly.

```json
{
  "spec": 1,
  "reset": "owned-v1",
  "tokens": {
    "spacingUnit": "0.25rem",
    "colors": {
      "accent": "var(--color-accent)",
      "white": "#ffffff",
      "neutral-50": "#fafafa",
      "neutral-100": "#f5f5f5",
      "neutral-200": "#e5e5e5",
      "neutral-300": "#d4d4d4",
      "neutral-400": "#a3a3a3",
      "neutral-500": "#737373",
      "neutral-600": "#525252",
      "neutral-700": "#404040",
      "neutral-800": "#262626",
      "neutral-900": "#171717",
      "neutral-950": "#0a0a0a",
      "sky-50": "#f0f9ff",
      "sky-300": "#7dd3fc",
      "sky-500": "#0ea5e9",
      "sky-700": "#0369a1",
      "sky-950": "#082f49",
      "emerald-50": "#ecfdf5",
      "emerald-300": "#6ee7b7",
      "emerald-500": "#10b981",
      "emerald-700": "#047857",
      "emerald-950": "#022c22",
      "violet-50": "#f5f3ff",
      "violet-300": "#c4b5fd",
      "violet-500": "#8b5cf6",
      "violet-700": "#6d28d9",
      "violet-950": "#2e1065",
      "amber-50": "#fffbeb",
      "amber-300": "#fcd34d",
      "amber-500": "#f59e0b",
      "amber-700": "#b45309",
      "amber-950": "#451a03",
      "rose-50": "#fff1f2",
      "rose-300": "#fda4af",
      "rose-500": "#f43f5e",
      "rose-700": "#be123c",
      "rose-950": "#4c0519"
    },
    "sizes": { "2xl": "42rem" },
    "fontSizes": {
      "xs": { "size": "0.75rem", "lineHeight": "1rem" },
      "sm": { "size": "0.875rem", "lineHeight": "1.25rem" },
      "lg": { "size": "1.125rem", "lineHeight": "1.75rem" },
      "2xl": { "size": "1.5rem", "lineHeight": "2rem" },
      "3xl": { "size": "1.875rem", "lineHeight": "2.25rem" }
    },
    "fontFamilies": { "mono": "ui-monospace, SFMono-Regular, Menlo, monospace" },
    "fontWeights": { "medium": "500", "semibold": "600" },
    "lineHeights": { "relaxed": "1.625" },
    "letterSpacings": { "tight": "-0.025em" },
    "radii": { "md": "0.375rem" }
  },
  "breakpoints": { "sm": { "minWidthPx": 640 } },
  "dark": { "attribute": "data-theme", "value": "dark" },
  "authoredClasses": { "prose": true }
}
```

There are 38 colour tokens, including the neutral values consumed only by authored CSS. Named spacing is unnecessary for this scaffold; all nonzero numeric spacing derives from the explicit unit. rounded-full is the language constant. max-w-2xl uses sizes. The scaffold does not use bare rounded, so no radii.default is declared.

Replace its three Tailwind constructs as follows; retain all ordinary authored selectors/declarations except the mechanical palette-variable change.

| Original construct | Replacement |
| --- | --- |
| `@import "tailwindcss";` | Remove; wind emits the prelude, selected reset and configured token layer. |
| `@custom-variant dark ...` | Remove; wind.dark has attribute data-theme and value dark. |
| `@theme { ... }` with accent/code-bg/code-fg | Replace with the root custom-property declarations below inside the existing base layer. |
| Existing `@layer base` and `@layer components` | Keep; the generated prelude assigns their order after reset/tokens/highlight. |
| All 27 references to `var(--color-neutral-N)` across 10 shades | Replace with `var(--zw-color-neutral-N)`; the palette table supplies each value. |
| Authored references to `--color-accent`, `--color-code-bg`, `--color-code-fg` | Keep; define them as below. |

```css
@layer base {
  :root {
    --color-accent: oklch(0.53 0.21 258);
    --color-code-bg: #2b303b;
    --color-code-fg: #c0c5ce;
    color-scheme: light;
  }
  [data-theme="dark"] {
    --color-accent: oklch(0.79 0.13 254);
    color-scheme: dark;
  }
}
```

Merge these declarations into the existing base block; preserve ::selection and its accent mix. The theme toggle sets data-theme on html, where the token alias also resolves. If nested theme scopes are introduced later, follow the token override rule above.

Owned reset acceptance for this migration: headings receive their specified text-size and paired leading, list bullets disappear in navigation/post lists but .prose lists explicitly restore disc/decimal, controls inherit font, bare border/top/bottom remain visible solid 1px, callout left border remains 4px, links inherit unless authored or utility decoration applies, and inline code retains its authored font and colour treatment. The margin-top sibling implementation preserves the scaffold's ordinary block-flow spacing; hidden/reversed-child edge cases are a deliberate language definition and require separate compiler fixtures. Default system font fallback and the explicit hex palette may differ slightly from the old output; these are intended choices to measure and record, not reasons to silently import the previous theme.

Rename the styling post to `content/blog/styling-with-zudo-wind.md`, update its links and pinned slug, and explain these owned values in original prose.

node-free receives no wind stylesheet after final cutover because absent wind has reset none and no tokens or candidates; explain that in its styling prose. During the temporary seam, use an explicit empty wind object only in its scratch verification copy as #3267 requests.

## Revision and implementation locks

| Revision | Date | Change |
| --- | --- | --- |
| 1 | 2026-09-28 | Initial ratification from ledger L and aggregate C/S evidence. Owns the grammar, reset, palette, cascade, source extension policy, configuration and manifests above. |
| 2 | 2026-09-28 | Foreground review: disambiguate unsupported-syntax diagnostic precedence, correct R23 cross-reference, fix shared-root arbitrary-value and static-alias/declaration-order details; refresh the expanded issue locks. |

Revision 2 locks #3248–#3257, #3261, #3263–#3271, #3285, #3292–#3296, #3299, #3301, #3302, #3305, #3306, #3309 and #3310 under the expanded #3246 planner addendum. Their “Locked by the zudo-wind v1 spec” sections name the binding decision ids. Scheduling headers and every other body section are preserved. All implementations must read this spec under the authority order.

Required downstream verification: parser rejection fixtures per G/R id (#3248); config and token validation (#3249); executable catalog examples (#3250/#3251); exact reset/output goldens and permutation tests (#3252); extractor/source-index fixtures (#3253/#3254); schema/export/explain tests (#3255); browser computed values for spacing, variant specificity, noninherited translation, all resets and unlayered cascade (#3257); real clean/warm builds, manifest ownership and authored/package assets (#3269). No browser, compiler or performance claim is certified by this document-only decision task.

## Later compatibility policy (2026-10-06)

The separate [Wind preset-free compatibility profile](wind-compatibility-profile.md)
(#3826) defines the bounded compatibility promise and evidence requirements. It
preserves this document's historical revision and owned-language boundary while
allowing reviewed native utility adoptions. Its version is independent of language
spec/catalog revisions; it does not certify this spec's historical verification.
