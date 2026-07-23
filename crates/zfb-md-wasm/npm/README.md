# @takazudo/zfb-md-wasm

A WebAssembly build of [zfb](https://github.com/Takazudo/zudo-front-builder)'s
markdown / MDX → JavaScript conversion pipeline, for **browser-side dynamic
conversion**. The headline use case is **CMS live preview**: convert MDX to a
runnable ES module (or markdown to HTML) in the browser, on every keystroke,
with output **parity** to what zfb produces at build time.

Parity is the reason this package exists instead of running
[`@mdx-js/mdx`](https://mdxjs.com/) in the browser: `@mdx-js/mdx` is a
_different_ pipeline, so its preview would not match zfb's built output.
`zfb-md-wasm` compiles the same Rust pipeline zfb itself uses, so a preview is
faithful to the real thing.

Node ≥ 20 can also load the package (this is what the test suite uses), but the
browser is the target. A server with shell access should keep calling the `zfb`
binary directly.

## Install

```sh
pnpm add @takazudo/zfb-md-wasm
```

## Two API tiers

Both functions take the markdown/MDX `source` and an options object (every
field optional; `{}` selects all defaults). Both return a result object plus a
`diagnostics` array — **expected failures come back as diagnostics, never as a
thrown error** (see the trap contract below for what _does_ throw).

### Diagnostic location contract

`Diagnostic.message` is opaque display text. Do not parse or rewrite it:
upstream markdown-rs prose can embed a related coordinate (for example, an
MDX opener location), and that coordinate is not part of this package's public
contract and may use the dependency's coordinate space. The structured
`line`/`column` pair is the sole supported diagnostic location. For
`"markdown"` and `"frontmatter"` diagnostics it is 1-based in the original
source's JavaScript UTF-16 code units (including frontmatter); for `"options"`
it refers to the options JSON document. Either field is `null` when no
structural location is available.

### `compile(source, options?)` — MDX → ES-module JS

Full MDX → JSX → SWC → ES module. The emitted module has a `MDXContent`
default export (a component function) using the automatic JSX runtime.

```ts
import { compile } from "@takazudo/zfb-md-wasm";

const { code, frontmatter, diagnostics } = await compile(
  "---\ntitle: Hello\n---\n\n# Welcome\n\n<Callout>Sum is {1 + 2}</Callout>\n",
  { filename: "post.mdx", jsxRuntime: "preact" },
);
// code        -> ES-module JS source (string) or null on failure
// frontmatter -> { title: "Hello" }
// diagnostics -> []
```

Frontmatter values are returned in the `frontmatter` field — they are **not**
exposed as an in-content binding. The compiled module has no `frontmatter`
variable in scope, so a `{frontmatter.title}` reference inside the source
would throw `ReferenceError` at runtime; read the values from the result
object instead.

### `renderHtml(source, options?)` — markdown → HTML

Markdown → hast → HTML string, **skipping SWC at runtime**. Use this for a
plain-markdown preview when you don't need to evaluate a component module.

```ts
import { renderHtml } from "@takazudo/zfb-md-wasm";

const { html, frontmatter, diagnostics } = await renderHtml("# Heading\n\nSome **bold** text.\n", {
  filename: "post.md",
});
// html -> "<h1>Heading</h1><p>Some <strong>bold</strong> text.</p>"
```

`renderHtml` accepts and ignores `jsxRuntime` / `development`, so one options
object can serve both tiers.

### `version()` / `init()`

`version()` returns the package version for host-side compatibility checks.
Published artifacts are stamped with the release semver at build time; local
development builds fall back to the Rust manifest placeholder.
`init()` eagerly loads and instantiates the wasm module; it's optional (every
call instantiates on first use) but useful to front-load the one-time
fetch/compile cost at app startup.

## `highlightCode(code, options)` — direct semantic HTML

`highlightCode()` highlights arbitrary source directly; it does **not** need a
Markdown fence or run the MDX/SWC pipeline. It is intentionally closed to
semantic class mode, so the returned HTML never carries inline colours or
Shiki classes.

If `highlightCode` is the only thing you use, import it from
`@takazudo/zfb-md-wasm/highlight` instead of the package root — that entry
ships a separate, much smaller wasm artifact with no `compile`/`renderHtml`
(and no md/MDX/JSX pipeline behind them at all). Same function, same result
shape, same `init()`/`version()`; see "Artifact size" below for the byte
savings.

```ts
import {
  highlightCode,
  type HighlightCodeOptions,
  type HighlightCodeResult,
} from "@takazudo/zfb-md-wasm";
// or, for the smaller highlight-only artifact:
// } from "@takazudo/zfb-md-wasm/highlight";

const output: HighlightCodeResult = await highlightCode("const answer = 42;", {
  language: "javascript", // required
  mode: "class", // optional; the only accepted mode
  classPrefix: "hi-", // optional; the default
  roleClasses: { keyword: "text-violet-600 dark:text-violet-400" },
} satisfies HighlightCodeOptions);

// output.html:
// <pre class="hi-root"><code><span class="line"><span class="text-violet-600 dark:text-violet-400">const</span> …</span></code></pre>
```

The direct types are:

```ts
type HighlightRole =
  | "escape"
  | "operator"
  | "comment"
  | "string"
  | "number"
  | "constant"
  | "keyword"
  | "function"
  | "type"
  | "namespace"
  | "property"
  | "variable"
  | "tag"
  | "attribute"
  | "punctuation"
  | "inserted"
  | "deleted"
  | "heading";

interface HighlightCodeOptions {
  language: string;
  mode?: "class";
  classPrefix?: string;
  roleClasses?: Partial<Record<HighlightRole, string>>;
}
interface HighlightCodeResult {
  html: string | null;
  diagnostics: HighlightDiagnostic[];
}
interface HighlightDiagnostic {
  severity: "error" | "warning";
  source: "options" | "highlight" | "internal";
  message: string;
  line: number | null;
  column: number | null;
}
```

The canonical default wrapper is `<pre class="hi-root"><code>…</code></pre>`;
each non-empty source line is a `<span class="line">`. With the default
`classPrefix: "hi-"`, the full-name role keys map to classes: `escape` →
`hi-esc`, `operator` → `hi-op`, `comment` → `hi-com`, `string` → `hi-str`,
`number` → `hi-num`, `constant` → `hi-const`, `keyword` → `hi-kw`, `function`
→ `hi-fn`, `type` → `hi-ty`, `namespace` → `hi-ns`, `property` → `hi-prop`,
`variable` → `hi-var`, `tag` → `hi-tag`, `attribute` → `hi-attr`,
`punctuation` → `hi-punct`, `inserted` → `hi-ins`, `deleted` → `hi-del`, and
`heading` → `hi-hd`. `roleClasses` uses those **full names**, not suffixes:
`{ keyword: "my-keyword" }` replaces `hi-kw`; `{ kw: "…" }` is an option
error. A custom `classPrefix: "token-"` makes the root `token-root` and the
unoverridden keyword class `token-kw`.

Malformed/directly invalid options — missing or empty `language`, unsupported
`mode`, invalid prefix, invalid role key, or extra fields — return `html:
null` with an `error` diagnostic from `source: "options"`; they do not throw.
An unknown non-empty language instead returns escaped fallback markup and one
`warning` diagnostic from `source: "highlight"` with `line`/`column` `null`.
This lets an editor show literal `<tag>&` safely while retaining a useful
warning. Empty and incomplete HTML/CSS/JavaScript editor input are accepted
and can produce normal class markup without a diagnostic.

## `parseToAst(source, options?)` — markdown/MDX → raw mdast

`parseToAst()` parses markdown/MDX source into a serialized **mdast** tree —
raw markdown-rs output converted into an open carrier that can optionally
hold generic directive nodes, before any zfb visitor runs. Use it when a
consumer needs the _tree_, not rendered output: a live
preview that wants remark/unist-ecosystem tooling (`mdast-util-to-hast`, a
custom mdast transform, editor↔preview scroll sync keyed on source
positions) instead of zfb's own HTML/JS rendering.

```ts
import { parseToAst, type ParseToAstResult } from "@takazudo/zfb-md-wasm";

const { ast, frontmatter, diagnostics }: ParseToAstResult = await parseToAst(
  "---\ntitle: Hello\n---\n\n# Welcome\n\nSome *emphasis*.\n",
  { filename: "post.mdx" },
);
// ast         -> the mdast root, or null on failure
// frontmatter -> { title: "Hello" }
// diagnostics -> []
```

`parseToAst` has its own closed options document:

```ts
interface ParseToAstOptions {
  filename?: string; // lowercase .md/.mdx only; default <anonymous>.mdx
  dialect?: "markdown" | "mdx";
  directives?: boolean; // default false
  frontmatter?: "extract" | "node" | "none"; // default extract
  pipeline?: {
    gfm?: {
      strikethrough?: boolean; // default true
      table?: boolean; // default true
      autolinkLiteral?: boolean; // default false
      taskListItem?: boolean; // default false
      footnoteDefinition?: boolean; // default false; controls definitions + references
    };
  };
}
```

Frontmatter handling is explicit and parse-only:

| policy                | parsed source                                                       | AST YAML node                         | returned `frontmatter`              | malformed/unterminated YAML                                             |
| --------------------- | ------------------------------------------------------------------- | ------------------------------------- | ----------------------------------- | ----------------------------------------------------------------------- |
| `"extract"` (default) | body after the recognized fence                                     | none                                  | parsed JSON, `null` if absent/empty | `ast: null`, `frontmatter: null`, one `frontmatter` diagnostic          |
| `"node"`              | full logical source, with only markdown-rs YAML frontmatter enabled | canonical `yaml` node when recognized | same parsed JSON as `extract`       | same failure as `extract`; no partial AST                               |
| `"none"`              | full logical source, YAML recognition disabled                      | none                                  | always `null`                       | no YAML diagnostic; the selected Markdown/MDX dialect parses every byte |

Recognition in `extract` and `node` is deliberately zfb-owned and YAML-only:
after one optional UTF-8 BOM, the document must start with `---` plus LF or
CRLF and close on a line exactly `---` plus LF, CRLF, or EOF. Empty YAML maps
to `null`. In `node`, `yaml.value` excludes fences and their line endings;
its position covers the complete fenced block. A leading BOM is ignored as
syntax in every policy but remains in original coordinates. `none` performs
no hidden extraction at all.

Frontmatter policy, dialect, and directives are orthogonal. Directives never
claim bytes inside a `yaml` node. Invalid policy strings/types return one
`options` diagnostic with `ast` and `frontmatter` both `null`; `compile` and
`renderHtml` reject this parse-only option. All exported unist offsets and
columns are original-source UTF-16 units, while `_markdownRsStops` remain
absolute original-source UTF-8 byte offsets.

With `directives: true`, generic remark-directive syntax is parsed before any
zfb visitor can run. The raw tree can contain `containerDirective`,
`leafDirective`, and `textDirective` nodes with `name`, always-present
`attributes`, `children`, and `position`. Container labels are paragraph
children carrying `data: { directiveLabel: true }`. Boolean attributes use
the empty string. No directive registry, component mapping, or expansion is
applied. With the option absent or false, directive-looking source remains
the same literal paragraph/MDX-expression output as before and incurs no
directive parse pass.

Without `dialect`, a `.md` filename selects Markdown/CommonMark and `.mdx`
selects MDX. Omitting `filename` uses `<anonymous>.mdx`, so the default is
MDX. An explicit dialect is authoritative for either valid extension (for
example `{ filename: "preview.mdx", dialect: "markdown" }`), but it does not
allow any other extension. `filename: null`, `dialect: null`, invalid enum
strings/types, compile-only keys (`jsxRuntime`, `development`), other pipeline
keys, and unknown keys return one `options` diagnostic with `ast` and
`frontmatter` both `null`; they never trap.

Markdown mode keeps CommonMark HTML/comments, angle autolinks, indented code,
and literal braces (`![alt](x.png){w=full}` leaves `{w=full}` as text). MDX
mode keeps JSX and expression parsing and its conflicting HTML/autolink/
indented-code exclusions. The five GFM switches above are independent of the
dialect and have the same defaults in both modes. Math remains disabled.

`ast` is `MdastRoot | null` — a real TypeScript union over the mdast node
set markdown-rs emits (`Root`, `Paragraph`, `Heading`, `Text`, `List`,
`Link`, `MdxJsxFlowElement`, …; see `types.ts`'s `MdastNode`), plus a
`{ type: string; position: AstPosition; [key: string]: unknown }` catch-all
so an unrecognized/future node type stays TYPED (never collapses to a bare
`unknown`) rather than being dropped from the union. Narrow on `type` to
select a node kind:

```ts
for (const child of ast?.children ?? []) {
  if (child.type === "heading") {
    // `child.position` is fully typed. Because the union includes the
    // open `{ type: string; … }` catch-all (its `type` overlaps every
    // literal), a `type` narrow can't *exclude* it, so kind-specific
    // fields like `depth` resolve through the catch-all's index
    // signature as `unknown` — assert the node kind to read them:
    const heading = child as Heading;
    console.log(heading.depth, heading.position.start.line);
  }
}
```

The catch-all is a deliberate tradeoff: it keeps forward-compatibility
(new markdown-rs node types stay typed) at the cost of the clean
discriminated-union narrowing you'd get from a closed union. `position`
and `type` are always available without a cast; kind-specific fields
need the assertion above.

### Validated mdast/unified adapter

Use `toMdastRoot` when passing the result to the mdast/unified ecosystem. It
validates the complete tree and returns a detached value assignable directly
to `mdast.Root`, with the canonical MDX and remark-directive content models:

```ts
import type { Root } from "mdast";
import { parseToAst, toMdastRoot } from "@takazudo/zfb-md-wasm";

const result = await parseToAst("# Welcome\n", { filename: "post.md" });
const root: Root = toMdastRoot(result.ast);

for (const node of root.children) {
  if (node.type === "heading") {
    console.log(node.depth); // narrowed without an assertion
    node.data ??= {};
    node.data.hName = `h${node.depth}`;
  }
}
```

The adapter recursively checks required fields, positions, scalar values,
directive/JSX attributes, and each parent's allowed child model. Unknown or
currently unsupported node types (including math, TOML, and `mdxjsEsm`) are
never dropped: `toMdastRoot` throws `MdastAdapterError`, whose `path` and
`nodeType` identify the failing value. A `null` parse result throws at `$`.
Check `diagnostics` first when normal parse failures are expected.

The returned tree is a deep clone. Internal `_markdownRsStops` coordinates
are validated and omitted, and an omitted MDX JSX fragment name is normalized
to canonical `null`. The broad raw `MdastRoot` remains available for consumers
that intentionally handle future or custom nodes without this closed-world
validation.

The tree is **mdast, not hast**: block/inline structure (`heading`,
`paragraph`, `emphasis`, `list`, …), not an HTML-shaped tree. It is the
**raw parser output** — with source selection controlled by the frontmatter
policy above, and **pre-visitor**: no `githubAlerts`/directive rewriting, no
zfb-synthesized node types. MDX JSX elements and expressions survive exactly as
markdown-rs parsed them (`mdxJsxFlowElement`/`mdxJsxTextElement` carry
`name`/`attributes`/`children`; `:::note`-style directive text and
GitHub-style `> [!NOTE]` alerts stay plain paragraph/blockquote text,
because rewriting them is a visitor's job this tier deliberately skips).

### Position contract: UTF-16 code units

Every real tree node carries `position: { start, end }`, each an
`{ line, column, offset }` point. `line` is 1-based and unit-agnostic;
`column` (1-based) and `offset` (0-based) are **UTF-16 code units** — the
same indexing `String.prototype.slice`, `mdast-util-to-hast`, and
remark/unist tooling already use, so positions from `parseToAst` slot
directly into that ecosystem without a conversion step of your own. This
matters once source is non-ASCII: a scalar value outside the Basic
Multilingual Plane (most emoji) needs a UTF-16 surrogate pair, so it
advances `offset`/`column` by 2 units, not 1 (and not the 4 UTF-8 bytes it
takes on disk). Frontmatter lines are included in `line`/`offset`, same as
`compile`/`renderHtml`'s diagnostics.

### Documented divergences from remark-parse / remark-mdx

- `mdxJsxAttribute` records (a JSX element's individual attributes) carry
  no `position` — markdown-rs does not model attribute positions.
- Top-level `import`/`export` degrade to plain paragraphs (no `mdxjsEsm`
  nodes), and MDX expressions carry no estree data — the wasm boundary
  cannot host a JS ESM/acorn parser. Keep using remark directly for
  documents that need remark-mdx-equivalent ESM/estree.
- A `_markdownRsStops` field on MDX expression/ESM nodes is
  markdown-rs-internal re-parse bookkeeping: internal, unstable, and
  **byte**-based (unlike `position`) — never slice a string with it.

### Why (and when) to reach for it

`parseToAst` exists for the same reason `compile`/`renderHtml` do: parity
with zfb's own pipeline, in the browser, on every keystroke. A Node
benchmark (`parseToAst` + `JSON.parse`, through the real built package,
against `remark-parse` under a capability-matched config — full method in
`test/bench/bench-parse-ast.mjs`) shows a consistent win at the document
sizes a live preview actually parses:

| doc             | bytes  | remark-parse mean | parseToAst+JSON.parse mean | mean win |
| --------------- | ------ | ----------------- | -------------------------- | -------- |
| small (~1.4 KB) | 1,361  | 0.70 ms           | 0.29 ms                    | ~2.4x    |
| medium (~21 KB) | 21,846 | 12.06 ms          | 6.24 ms                    | ~1.9x    |

(One-off wasm init — fetch/compile/instantiate, paid once — was 16.1 ms in
that run; steady-state numbers above never include it.) On a CJK-heavy
(Japanese prose + emoji) document the win narrows to roughly parity
(~1.0–1.3x across repeated runs) — the UTF-16 position conversion has a
real, measurable cost on non-ASCII-heavy input, unlike pure-ASCII sources,
which take a fast path that skips the conversion (and the corresponding
Rust benchmark for that fixture) entirely. Numbers vary by machine/Node
version; re-run `node test/bench/bench-parse-ast.mjs` (after `pnpm build`)
for your own environment.

## Browser loading and emitted resources

The package root has a `browser` export condition. Its browser entry imports
the generated glue and Wasm binary through an explicit bundler `?url` asset
contract. Vite and zfb's pinned esbuild setup therefore keep separate resource
edges for exactly `zfb_md_wasm_glue.zfb-resource.mjs` and
`zfb_md_wasm_bg.wasm`; a zfb production build emits them under hashed names:

```text
assets/islands-resource-zfb_md_wasm_glue.zfb-resource-<hash>.mjs
assets/islands-resource-zfb_md_wasm_bg-<hash>.wasm
```

Keep the package import in a user action when first-load cost matters:

```ts
button.addEventListener("click", async () => {
  const { highlightCode } = await import("@takazudo/zfb-md-wasm");
  const result = await highlightCode(editor.value, { language: "javascript" });
  preview.innerHTML = result.html ?? "";
});
```

That import/call boundary keeps both resources unloaded before the action;
the first public API call fetches the glue and wasm. Your production server
must serve the generated `.mjs` with `application/javascript` and `.wasm`
with `application/wasm`. Consume the packed package/browser entry — do not
replace the static imports with source paths or manually copied resource
files, which breaks the emitted URL graph. No Vite plugin, alias, or
package-specific consumer configuration is required.

The `./highlight` subpath (see "Artifact size" above) has the identical
`browser` export condition and resource-loading contract, pointed at its own
separate resources: `zfb_md_wasm_highlight_glue.zfb-resource.mjs` and
`zfb_md_wasm_highlight_bg.wasm`. Importing `@takazudo/zfb-md-wasm` and
`@takazudo/zfb-md-wasm/highlight` in the same bundle loads BOTH wasm
artifacts — pick one entry per bundle.

## Options shape

```ts
interface ZfbMdWasmOptions {
  filename?: string; // must end .md/.mdx; drives frontmatter dispatch + diagnostics
  jsxRuntime?: "preact" | "react"; // compile only; default "preact"
  development?: boolean; // compile only; default false
  pipeline?: {
    // A syntect theme name. Absent, or explicit `null`, keeps the built-in
    // default theme (`base16-ocean.dark`) -- fenced code is ALWAYS
    // highlighted through this field; there is no "no syntax highlighting"
    // value. Mutually exclusive with `codeHighlight.mode: "class"`.
    theme?: string | null;
    gfm?: {
      strikethrough?: boolean;
      table?: boolean;
      autolinkLiteral?: boolean;
      taskListItem?: boolean;
      footnoteDefinition?: boolean;
    };
    cjkFriendly?: boolean;
    hardBreaks?: boolean;
    // Output mode + class-mode knobs for fenced-code highlighting -- see
    // "codeHighlight: inline vs. class mode for compile/renderHtml" below.
    codeHighlight?: {
      mode?: "inline" | "class"; // default "inline"
      classPrefix?: string; // default "hi-"; class mode only
      roleClasses?: Partial<Record<HighlightRole, string>> | null; // class mode only
    } | null;
    features?: Record<string, unknown>; // zfb's MarkdownFeaturesConfig, verbatim
  };
}
```

`pipeline` is zfb's **resolved features config** as JSON — the same shape zfb
derives from `zfb.config.ts` at build time. See "Limitations" for why it's
resolved JSON rather than a config file. Unknown fields are rejected at both
nesting levels (an `options`-source diagnostic).

This options document is for `compile` and `renderHtml`. `parseToAst` uses the
distinct closed `ParseToAstOptions` shown in its section above; it rejects
visitor/serializer and compile-only knobs rather than silently ignoring them.

### `codeHighlight`: inline vs. class mode for `compile`/`renderHtml`

`pipeline.codeHighlight` controls how `compile()` and `renderHtml()` highlight
**fenced code blocks** in markdown/MDX source — a separate knob from the
direct `highlightCode()` function above, but they share the same class-mode
output contract: the same `hi-` prefix, the same 18-role `HighlightRole`
taxonomy, the same `classPrefix`/`roleClasses` shape, so **one stylesheet
covers both APIs** when both run in class mode.

- **Absent, `null`, or `{ mode: "inline" }`** (the default) reproduces the
  pre-existing per-token inline-color behaviour byte-for-byte: `<pre
class="syntect-{theme-slug}">` with `<span style="color:#…;">` tokens. This
  mode is where `pipeline.theme` applies.
- **`{ mode: "class" }`** switches fenced code to the same `hi-root` /
  `hi-{role}` semantic class markup `highlightCode()` produces — no inline
  colours, so **who owns color is the host page's stylesheet**, not the wasm
  output. `classPrefix`/`roleClasses` behave exactly like `highlightCode()`'s
  options (see above for the full role → default-class table).
- **`mode: "class"` is mutually exclusive with a _non-null_ top-level
  `theme`** — themes don't affect class emission, so naming one alongside
  `mode: "class"` returns an `options`-source diagnostic
  (`codeHighlight.mode "class" is mutually exclusive with theme`) rather than
  silently ignoring it. `theme: null` (or omitting `theme` entirely) is not a
  conflict — `null` deserializes to "absent", the same as leaving the field
  out — so `{ theme: null, codeHighlight: { mode: "class" } }` is accepted.
- In `compile()`'s output, class mode changes only the emitted class names —
  the surrounding shape (a `<pre>`/`<code>`/per-line `<span>` JSX tree with
  the token markup wrapped in `dangerouslySetInnerHTML`) is unchanged from
  inline mode.

````ts
import { renderHtml } from "@takazudo/zfb-md-wasm";

const { html } = await renderHtml("```js\nconst x = 1;\n```\n", {
  pipeline: { codeHighlight: { mode: "class" } },
});
// html -> `<pre class="hi-root"><code><span class="line">
//           <span class="hi-kw">const</span> x = <span class="hi-num">1</span>;
//         </span></code></pre>` (whitespace added for readability)
````

## Evaluating compiled modules in a browser

`compile()` returns ES-module _source_. To run it, turn it into a module (a
blob URL is the usual trick) and dynamic-import it:

```ts
const { code, diagnostics } = await compile(source, { filename: "preview.mdx" });
if (code === null) {
  // Compilation failed — render `diagnostics` instead of a module.
  return;
}
const url = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
const { default: MDXContent } = await import(/* @vite-ignore */ url);
URL.revokeObjectURL(url);
// render <MDXContent components={{ Callout }} /> with your framework
```

Pass your PascalCase components (the `<Callout>` in the source above) through
the module's `components` prop.

### ⚠️ You must supply the JSX runtime — and the preact case needs one alias

The compiled module imports its JSX runtime by bare specifier
(`preact/jsx-runtime` or `react/jsx-runtime`), so the page must resolve those —
via an [import map](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/script/type/importmap)
or your bundler.

**There is one asymmetry to know about.** zfb's emitter takes the JSX _factory_
from your chosen runtime but **always imports `Fragment` from
`react/jsx-runtime`**, regardless of `jsxRuntime`. This is zfb's production
emitter shape (so it's parity-correct, not a bug) — but it means a **preact**
consumer must alias `react/jsx-runtime` onto preact's, or `Fragment` will fail
to resolve at runtime:

```html
<script type="importmap">
  {
    "imports": {
      "preact/jsx-runtime": "https://esm.sh/preact/jsx-runtime",
      "react/jsx-runtime": "https://esm.sh/preact/jsx-runtime"
    }
  }
</script>
```

A `react` consumer just maps `react/jsx-runtime` to React's own and needs no
alias.

## Node usage

For tests and tooling, the package loads and runs under Node ≥ 20 with no extra
setup — the same `compile` / `renderHtml` / `parseToAst` / `version` API. This
is exactly how this package's own vitest suite (and `parseToAst`'s Node
benchmark against `remark-parse`) exercises the wasm.

## Parity guarantee & limitations

Output matches zfb's native pipeline on a fixed fixture corpus (the parity
suite gates exact-match). Deliberate limitations of the browser build:

- **Filesystem-bound plugins are inert.** `transclude`, `imageDimensions`, and
  `linkValidation` are registered but never touch a filesystem (there is none),
  exactly as zfb's own MDX loader runs them with build-context roots unarmed.
  Host-callback versions are a possible future epic.
- **Config is resolved JSON, not `zfb.config.ts`.** Evaluating a TypeScript
  config needs a JS engine; that stays build-side. Resolve your config to JSON
  first and pass it as `pipeline`.
- **No cross-file features.** Route-table link resolution and cross-file anchor
  resolution need the whole project graph, which a single-document browser call
  doesn't have.
- **The default artifact carries SWC even for `renderHtml`-only use.** One
  cdylib can't tree-shake SWC away when only `renderHtml` is called; a slim
  `renderHtml`-only artifact is a documented possible follow-up. If you only
  ever call `highlightCode`, use the `./highlight` entry instead (see
  "Artifact size" below) — it drops `compile`/`renderHtml` and the whole
  md/MDX/JSX pipeline entirely.
- **Grammar subsetting is not built.** Both artifacts ship every bundled
  syntect grammar; there is no per-language allowlist knob.
- **Syntax highlighting uses syntect's `fancy-regex` backend** (native zfb uses
  `oniguruma`, which can't compile to wasm). The two are byte-identical on
  zfb's fixture corpus; any grammar-level divergences are tracked in the
  crate's informational backend-divergence test.

## Artifact size

Shipping SWC in the bytes makes the default module large. The build applies a
size-optimized cargo profile (`opt-level = "z"`, LTO, one codegen unit,
`panic = "abort"`) plus `wasm-opt`, which roughly halves the raw binary either
way. The package ships **two** wasm artifacts (zfb#1849, epic zfb#1845):

| Entry          | Import                            | What it has                                           | Raw `.wasm` | Gzipped |
| -------------- | --------------------------------- | ----------------------------------------------------- | ----------- | ------- |
| Default        | `@takazudo/zfb-md-wasm`           | `compile` + `renderHtml` + `highlightCode`            | ~2.9 MB     | ~1.3 MB |
| Highlight-only | `@takazudo/zfb-md-wasm/highlight` | `highlightCode` only (no md/MDX/JSX pipeline, no SWC) | ~1.4 MB     | ~0.7 MB |

The highlight-only artifact drops the `pipeline` Cargo feature entirely (see
`crates/zfb-md-wasm/Cargo.toml`) rather than subsetting syntect grammars —
both artifacts bundle every grammar (see "Grammar subsetting is not built"
above). The CI `wasm-md` job prints the authoritative gzipped size for BOTH
artifacts on every run — treat that as the source of truth rather than this
table, which can drift.

## Error / trap / re-init contract

- **Expected failures never throw.** Parse errors, malformed options JSON,
  unknown themes, a bad filename — all come back as structured `Diagnostic[]`
  entries (`{ severity, source, message, line, column }`) on a normal result,
  with `code` / `html` set to `null`. `line`/`column` are 1-based; for
  `markdown` / `frontmatter` sources they point into the original source, for
  `options` into the options JSON.
- **A wasm _trap_ is always a bug, and it poisons the instance.** On
  `wasm32-unknown-unknown` a Rust panic (or other internal fault) lowers to a
  wasm trap; `catch_unwind` is not reliable recovery, so the instance is dead
  afterward. This wrapper handles that for you: it catches the
  `WebAssembly.RuntimeError`, **drops the poisoned instance and re-instantiates
  a fresh one in the background** (from the cached compiled module, so no
  recompile), and throws a `ZfbMdWasmTrapError` for that one call. The next
  `compile` / `renderHtml` / `highlightCode` / `version` call transparently
  uses the fresh instance. The API is stateless per call, so re-init is
  lossless. In a browser the wrapper keeps the compiled `WebAssembly.Module`,
  imports a fresh glue URL with `?zfbMdWasmGen=N`, and re-instantiates without
  a second wasm download/compile; recovery is bounded to 16 attempts to avoid
  unbounded ES module records.

  If you ever see a `ZfbMdWasmTrapError`, please report it with the input that
  triggered it — the crate is designed never to trap on structured input.
  (Fuzzing the trap surface is a documented follow-up.)

## License

MIT © Takeshi Takatsudo
