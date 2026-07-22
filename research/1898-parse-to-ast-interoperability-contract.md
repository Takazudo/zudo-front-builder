# `parseToAst` interoperability contract (#1898)

Status: locked for implementation by issues #1902, #1904, #1906, #1907, and #1908.

This note is the durable architecture decision for the raw `parseToAst` tier.
It does not change `compile` or `renderHtml` behavior. The contract starts from
the `0.1.0-next.92` compatibility baseline: raw pre-visitor MDX output,
frontmatter extraction, original-source UTF-16 unist positions, and byte-based
`_markdownRsStops` all remain unchanged unless a mode below explicitly says
otherwise.

## Public options boundary

`parseToAst` gets its own closed options document. It no longer deserializes
through the shared `WasmOptions`, because most of `WasmOptions.pipeline` and
all compile-only fields are currently accepted and then ignored by raw parsing.

```json
{
  "filename": "posts/hello.md",
  "dialect": "markdown",
  "directives": false,
  "frontmatter": "extract",
  "pipeline": {
    "gfm": {
      "strikethrough": true,
      "table": true,
      "autolinkLiteral": false,
      "taskListItem": false,
      "footnoteDefinition": false
    }
  }
}
```

All fields are optional. The TypeScript declaration is:

```ts
export type ParseDialect = "markdown" | "mdx";
export type FrontmatterPolicy = "extract" | "node" | "none";

export interface ParsePipelineOptions {
  gfm?: GfmOptions;
}

export interface ParseToAstOptions {
  /** Must end in `.md` or `.mdx`; default: `<anonymous>.mdx`. */
  filename?: string;
  /** Default is inferred from filename: `.md` => markdown, `.mdx` => mdx. */
  dialect?: ParseDialect;
  /** Enable generic remark-directive syntax; default: false. */
  directives?: boolean;
  /** Frontmatter handling; default: "extract". */
  frontmatter?: FrontmatterPolicy;
  /** Raw-parser options only. */
  pipeline?: ParsePipelineOptions;
}
```

The Rust boundary mirrors it literally:

```rust
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ParseDialect { Markdown, Mdx }

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FrontmatterPolicy { #[default] Extract, Node, None }

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct ParsePipelineOptions { gfm: GfmOptions }

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct ParseToAstOptions {
    #[serde(default, deserialize_with = "deserialize_present_non_null")]
    filename: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_non_null")]
    dialect: Option<ParseDialect>,
    directives: bool,
    frontmatter: FrontmatterPolicy,
    pipeline: ParsePipelineOptions,
}

fn deserialize_present_non_null<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}
```

`dialect: None` is resolved only after the default/explicit filename is known;
the derived value is `markdown` for `.md` and `mdx` for `.mdx`. The enum's Rust
type has no default that could override filename inference. The custom field
deserializer is called only when the key is present: omission produces `None`,
while explicit `null`, a non-string filename, or a non-enum dialect rejects the
document. No TypeScript-optional field silently treats `null` as omission.

The result JSON and TypeScript shape stay source-compatible:

```ts
export interface ParseToAstResult {
  ast: MdastRoot | null;
  frontmatter: unknown;
  diagnostics: Diagnostic[];
}
```

markdown-rs's `mdast::Node` is a closed enum and therefore cannot contain new
directive children. The Rust/Wasm result must use a recursive serialization
carrier rather than claiming the final mixed tree is still that enum:

```rust
#[derive(Debug, Serialize, Deserialize)]
struct InteropMdastNode {
    #[serde(rename = "type")]
    kind: String,
    position: markdown::unist::Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    children: Option<Vec<InteropMdastNode>>,
    #[serde(flatten)]
    fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ParseToAstResult {
    ast: Option<InteropMdastNode>,
    frontmatter: serde_json::Value,
    diagnostics: Vec<Diagnostic>,
}
```

Base nodes pass through markdown-rs's own serde representation and are then
validated into this carrier; do not hand-maintain a second copy of every mdast
variant. `kind`, `position`, and recursive `children` are structural and typed;
variant-specific scalars, MDX attributes, `data`, and `_markdownRsStops` remain
lossless JSON fields. The carrier enables directive insertion at any content
depth and one recursive position/stops conversion without weakening the public
raw unknown-node tier.

### Entrypoint acceptance and rejection

| Key                                                                                       | `parseToAst`                        | `compile`                                    | `renderHtml`                                |
| ----------------------------------------------------------------------------------------- | ----------------------------------- | -------------------------------------------- | ------------------------------------------- |
| `filename`                                                                                | accepted; default `<anonymous>.mdx` | accepted; existing default `<anonymous>.mdx` | accepted; existing default `<anonymous>.md` |
| `pipeline.gfm`                                                                            | accepted                            | accepted                                     | accepted                                    |
| `dialect`                                                                                 | accepted, parse-only                | reject                                       | reject                                      |
| `directives`                                                                              | accepted, parse-only                | reject                                       | reject                                      |
| `frontmatter`                                                                             | accepted, parse-only                | reject                                       | reject                                      |
| `jsxRuntime`, `development`                                                               | reject                              | accepted with existing semantics             | accepted-and-ignored as today               |
| other `pipeline` keys (`theme`, `cjkFriendly`, `hardBreaks`, `codeHighlight`, `features`) | reject                              | accepted with existing semantics             | accepted with existing semantics            |
| any other key at any closed level                                                         | reject                              | reject                                       | reject                                      |

Rejection is a returned diagnostic, never a Wasm trap. Invalid JSON, wrong
types, unknown keys, invalid enum strings, and a filename without an exact
lowercase `.md` or `.mdx` suffix return:

```json
{
  "ast": null,
  "frontmatter": null,
  "diagnostics": [
    {
      "severity": "error",
      "source": "options",
      "message": "invalid parseToAst options JSON: ...",
      "line": 1,
      "column": 1
    }
  ]
}
```

Serde's location is used when available; semantic filename failures have null
line/column. No source parsing or frontmatter extraction happens after an
options failure. `compile` and `renderHtml` keep their current result shapes,
defaults, and diagnostics while their deserializer remains closed to the three
parse-only keys.

## Dialect contract

`markdown` starts from `markdown::ParseOptions::default()` / CommonMark and
then sets the five independently resolved GFM constructs. `mdx` starts from
`markdown::ParseOptions::mdx()` and applies those same five toggles. For either
base, `footnoteDefinition` controls both markdown-rs's definition and inline
label-start constructs. Math stays off.

| Filename         | `dialect` absent        | `dialect: "markdown"` | `dialect: "mdx"`   |
| ---------------- | ----------------------- | --------------------- | ------------------ |
| `*.md`           | Markdown                | Markdown              | MDX                |
| `*.mdx`          | MDX                     | Markdown              | MDX                |
| absent           | MDX (`<anonymous>.mdx`) | Markdown              | MDX                |
| any other suffix | options diagnostic      | options diagnostic    | options diagnostic |

An explicit dialect override is authoritative but does not waive the filename
extension gate. In Markdown, CommonMark HTML, comments, angle autolinks, and
indented code stay enabled; braces such as `{w=full}` are text. In MDX, the
current JSX/expression behavior and current diagnostics stay the baseline;
HTML, CommonMark angle autolinks, and indented code retain markdown-rs's MDX
semantics. The five GFM flags remain orthogonal in both modes and retain their
current conservative defaults: strikethrough/table on, the other three off.

The existing documented MDX divergences remain: without a JavaScript parser,
top-level ESM degrades to ordinary Markdown instead of `mdxjsEsm`, expression
nodes have no ESTree, JSX attribute records have no positions, fragments omit
`name`, and `_markdownRsStops` remain internal UTF-8 byte coordinates.

## Directive contract

`directives` defaults to `false`. False performs no directive scan or
allocation and preserves the current raw paragraph/MDX-expression output.
True enables generic directives in both dialects before any zfb visitor. It
does not consult `DirectiveRegistry`, map names to components, or run the
existing component-expansion visitor.

### Reference oracle

The exact oracle is the AST produced by these maintained major versions (the
versions probed for this decision are shown):

```js
// Markdown oracle
unified() // 11.0.5
  .use(remarkParse) // 11.0.0
  .use(remarkDirective) // 4.0.0
  .parse(source);

// MDX oracle
unified()
  .use(remarkParse) // 11.0.0
  .use(remarkMdx) // 3.1.1
  .use(remarkDirective) // 4.0.0
  .parse(source);
```

`remark-directive@4.0.0` resolves the syntax/conversion conventions from
`micromark-extension-directive@4.0.0` and `mdast-util-directive@3.x`. Fixture
comparison must pin these majors and compare all directive nodes, their
children/data/attributes, recovery behavior, and positions. Ordinary MDX
subtrees may differ only by the already documented zfb MDX divergences above.
An approximate node that merely looks similar is a failure.

### Runtime node shapes

```ts
export interface ContainerDirective {
  type: "containerDirective";
  position: AstPosition;
  data?: Record<string, unknown>;
  name: string;
  attributes: Record<string, string>;
  children: MdastNode[];
}

export interface LeafDirective {
  type: "leafDirective";
  position: AstPosition;
  data?: Record<string, unknown>;
  name: string;
  attributes: Record<string, string>;
  children: MdastNode[];
}

export interface TextDirective {
  type: "textDirective";
  position: AstPosition;
  data?: Record<string, unknown>;
  name: string;
  attributes: Record<string, string>;
  children: MdastNode[];
}
```

The Rust core shape before conversion into `InteropMdastNode` is:

```rust
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum DirectiveKind {
    ContainerDirective,
    LeafDirective,
    TextDirective,
}

#[derive(Debug, Serialize)]
struct DirectiveNode {
    #[serde(rename = "type")]
    kind: DirectiveKind,
    position: markdown::unist::Position,
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<InteropMdastNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<BTreeMap<String, serde_json::Value>>,
}
```

All three nodes have full original-source positions. `attributes` is always an
object; an absent attribute block yields `{}`. Parsed values are strings;
unquoted boolean attributes use `""`, not `null`. Character references and
escapes follow the oracle. A container label is its first `paragraph` child
with `data.directiveLabel === true`; leaf/text labels are direct phrasing
children. Missing labels yield no label children. Attribute records themselves
do not become tree nodes and have no positions.

Malformed syntax uses the oracle's recovery/literal-text behavior and does not
invent a directive-specific diagnostic. A base-dialect fatal error still
returns the normal `source: "markdown"` diagnostic.

### Rust parser strategy

markdown-rs 1.0 has no public syntax-extension hook, so this feature must not
be implemented by a JavaScript parser, a regex over an already-built tree, or
the current `DirectiveRegistry` visitor. Add a focused Rust module in
`zfb-content` that ports the directive name, label, attribute, leaf, text, and
container state machines from the pinned micromark extension and the lowering
rules from `mdast-util-directive`.

The module is a source-overlay parser:

1. Scan the original UTF-8 bytes with block/phrasing context and produce
   nested directive ranges plus exact byte spans. Failed constructs are left
   unclaimed so the base parser sees literal source.
2. Build a byte-length-preserving overlay for claimed directive syntax:
   replace claimed syntax bytes with ASCII spaces while retaining every LF/CR
   and retaining label/body bytes that markdown-rs must parse. This prevents
   MDX from consuming directive attribute braces and keeps offsets stable.
3. Parse ordinary regions and retained labels/bodies with the selected
   markdown-rs dialect/GFM construct set, then splice directive nodes into the
   recursive `InteropMdastNode` carrier under the proper block/phrasing content
   models. Nested containers recurse through the same range tree; no second
   interpretation of already-claimed syntax is allowed.
4. Assert every emitted span slices the original source and every child span
   is within its parent before returning the byte-positioned Rust tree.

The implementation may refine internal data structures, but changing this
grammar/oracle, falling back to post-hoc paragraph pattern matching, or adding
a JS runtime requires a new architecture decision.

## Frontmatter contract

`frontmatter` defaults to `"extract"`. Recognition in `extract` and `node` is
the existing zfb YAML form: after one optional leading UTF-8 BOM, the source
must begin with `---` followed by LF or CRLF; the closing line must be exactly
`---` followed by LF, CRLF, or EOF. Only YAML is recognized, never TOML.

| Policy    | Parser source                                          | YAML node                      | returned `frontmatter`                                         | malformed/unterminated YAML                                                       |
| --------- | ------------------------------------------------------ | ------------------------------ | -------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `extract` | body after closing fence                               | absent                         | parsed JSON value; `null` for no/empty block                   | `ast: null`, `frontmatter: null`, one `frontmatter` diagnostic (current behavior) |
| `node`    | full logical source with markdown-rs YAML construct on | present for a recognized block | same parsed JSON value as `extract`; `null` for no/empty block | same failure as `extract`; no partial AST escapes                                 |
| `none`    | full logical source with YAML construct off            | absent                         | always `null`                                                  | not a YAML error; the selected dialect parses the bytes as ordinary Markdown/MDX  |

`node` recognizes and parses YAML separately: zfb's extractor/`serde_yaml`
owns recognition and JSON conversion/diagnostics; markdown-rs owns the `yaml`
node and its source position. Its node value excludes both fences and their
line endings. `extract` continues parsing only the stripped body and shifts
all nodes back to original coordinates. `none` performs no YAML recognition,
so a leading `---` can become a thematic break and following lines can acquire
ordinary Markdown meaning.

One leading BOM is ignored as syntax in all policies but remains part of the
original coordinate space: the first syntactic character after it is UTF-16
offset 1, column 2. CRLF counts as two UTF-16 offsets and one line ending.
Frontmatter diagnostics continue pointing at original-source lines. An empty
body succeeds. In `extract`, its root is zero-width at the original body-start
point (including the closing-fence-at-EOF same-line column case); in `node`, a
present YAML node keeps the root spanning the YAML source; in `none`, the full
source determines the root span.

Directive and dialect processing applies to the selected parser source:
`extract` scans the body then shifts, while `node` and `none` scan the full
logical source. The YAML range in `node` is never eligible for directive
recognition.

## Position and serialization invariants

- Every real tree node has `position`; start is inclusive and end exclusive.
- Exported `line` is 1-based; `column` is 1-based UTF-16; `offset` is 0-based
  UTF-16, always against the complete original source including BOM and YAML.
- `source.slice(start.offset, end.offset)` must equal the node's original
  source span for every core, YAML, and directive node.
- `_markdownRsStops` remains the sole exception: tuple offsets are absolute
  UTF-8 bytes, shifted for extracted frontmatter but never UTF-16-converted.
- Node/browser exports and declarations serialize identically.

## Validated mdast adapter

The broad raw `MdastRoot`/`MdastNode` API remains the lossless, forward-
compatible Wasm serialization tier. It is intentionally not directly
assignable to ecosystem `mdast.Root` because `UnknownMdastNode` overlaps every
discriminant and parent children are broad. Add a validating, non-mutating
adapter instead of casts or global augmentation of the raw union.

```ts
import type { Root as EcosystemRoot } from "mdast";

export class MdastAdapterError extends TypeError {
  readonly path: string;
  readonly nodeType: string | null;
}

export function toMdastRoot(ast: MdastRoot | null): EcosystemRoot;
```

Accepting `null` lets consumers call `toMdastRoot(result.ast)` directly; null
throws `MdastAdapterError` at `$` with `nodeType: null`. The adapter recursively
validates required fields, scalar domains, positions, directive attributes,
MDX attribute records, and the canonical child content model at every parent.
It reports a JSONPath-like location and the observed node type. Examples:
`$.children[2].children[0]` and `unsupported mdast node type "futureNode" at
$.children[2]`.

The accepted runtime node set is the core/GFM/YAML nodes currently declared by
the package, the three locked directive nodes, and the currently emitted MDX
expression/JSX nodes. A `root` below `$`, math/TOML while those constructs are
off, `mdxjsEsm` while zfb cannot emit it, unknown/future types, a known type in
the wrong child content model, or malformed known fields all throw. Nodes are
never silently dropped. Consumers that intentionally handle such values keep
using the raw tier.

The return value is a deep canonical clone. Known optional serde fields may
remain absent where mdast permits absence. The required canonical MDX JSX
`name` is normalized from omitted to `null` for fragments and preserved for
named elements. Internal `_markdownRsStops` are validated as byte-offset pairs
on raw MDX records but omitted from the adapted clone. JSX attribute positions
and ESTree are not fabricated. `data`, when present, must be a non-null plain
object and is cloned; all known raw node/attribute declarations gain
`data?: Record<string, unknown>` so transforms can initialize or write
`node.data.hName`. Unexpected structural fields on a known raw node are
rejected; `data` is the extension channel.

Canonical TypeScript content models come from explicit package dependencies,
not duplicated local interfaces:

- runtime/package `dependencies`: `@types/mdast@^4`,
  `mdast-util-directive@^3.1`, and `mdast-util-mdx@^3` (the latter two export
  and register the canonical directive/MDX augmentations used in declarations);
- `devDependencies`: `mdast-util-to-hast`, `unist-util-visit`,
  `remark-directive@^4`, and other fixture-only tools;
- the adapter itself has no runtime import requirement beyond local code;
  type-only imports must remain type-only in emitted JavaScript.

`mdast.ts` must load the augmentation declarations explicitly so the packed
`.d.ts` is self-contained rather than relying on a consumer import order:

```ts
import type { Root as EcosystemRoot } from "mdast";
import type {} from "mdast-util-directive";
import type {} from "mdast-util-mdx";
```

Both `src/index.ts` and `src/browser.ts` export the same function, error class,
and types. Packed-tarball verification must install only the tarball in an
isolated consumer, resolve all declarations, assign the return directly to
`mdast.Root`, narrow headings by `type`, mutate `data.hName`, normalize both
named and fragment JSX, visit the tree, and pass a supported fixture through
`mdast-util-to-hast`. `./highlight` remains unchanged.

## Ownership boundaries

- #1902 owns dialect resolution and CommonMark/MDX construct composition.
- #1904 owns the Rust directive source parser, oracle corpus, and byte spans;
  it does not touch Wasm/npm exports.
- #1906 owns the default-off option wiring, directive serialization, coordinate
  conversion, raw TypeScript shapes, browser parity, and cost measurement.
- #1907 owns the three frontmatter policies, full-source parsing, BOM/CRLF and
  malformed-YAML behavior, and policy documentation.
- #1908 owns `toMdastRoot`, validation/normalization, canonical dependencies,
  declarations, package exports, and packed-consumer tests.

## Evidence and deliberate divergences

Repository evidence: `crates/zfb-md-wasm/src/lib.rs` currently shares
`WasmOptions`, always extracts frontmatter, and converts shifted raw positions;
`crates/zfb-content/src/facade.rs::parse_mdast` currently uses only
`PipelineOptions.gfm` and always starts from MDX; markdown-rs 1.0 exposes
CommonMark/MDX/GFM/frontmatter constructs but no public directive extension
hook; `npm/src/types.ts` preserves unknown nodes and omits fragment `name`.

Primary ecosystem references used for the oracle and canonical shapes:

- <https://github.com/remarkjs/remark-directive/tree/4.0.0>
- <https://github.com/micromark/micromark-extension-directive/tree/4.0.0>
- <https://github.com/syntax-tree/mdast-util-directive/tree/3.1.0>
- <https://github.com/mdx-js/mdx/tree/3.1.1/packages/remark-mdx>
- <https://github.com/syntax-tree/mdast-util-mdx/tree/3.0.0>
- <https://github.com/syntax-tree/mdast>

The locked divergences are intentional: no JavaScript/Acorn parser or ESTree,
no MDX ESM node, no JSX attribute positions, no TOML/math, no zfb directive
component expansion, original-source UTF-16 exported positions rather than
raw markdown-rs byte positions, and byte-only internal `_markdownRsStops`.
