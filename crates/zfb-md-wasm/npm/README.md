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

const { html, frontmatter, diagnostics } = await renderHtml(
  "# Heading\n\nSome **bold** text.\n",
  { filename: "post.md" },
);
// html -> "<h1>Heading</h1><p>Some <strong>bold</strong> text.</p>"
```

`renderHtml` accepts and ignores `jsxRuntime` / `development`, so one options
object can serve both tiers.

### `version()` / `init()`

`version()` returns the crate version (for host-side compatibility checks).
`init()` eagerly loads and instantiates the wasm module; it's optional (every
call instantiates on first use) but useful to front-load the one-time
fetch/compile cost at app startup.

## Options shape

```ts
interface ZfbMdWasmOptions {
  filename?: string;              // must end .md/.mdx; drives frontmatter dispatch + diagnostics
  jsxRuntime?: "preact" | "react"; // compile only; default "preact"
  development?: boolean;          // compile only; default false
  pipeline?: {
    theme?: string | null;        // a syntect theme name, or null for no highlighting
    gfm?: {
      strikethrough?: boolean; table?: boolean; autolinkLiteral?: boolean;
      taskListItem?: boolean; footnoteDefinition?: boolean;
    };
    cjkFriendly?: boolean;
    hardBreaks?: boolean;
    features?: Record<string, unknown>; // zfb's MarkdownFeaturesConfig, verbatim
  };
}
```

`pipeline` is zfb's **resolved features config** as JSON — the same shape zfb
derives from `zfb.config.ts` at build time. See "Limitations" for why it's
resolved JSON rather than a config file. Unknown fields are rejected at both
nesting levels (an `options`-source diagnostic).

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
setup — the same `compile` / `renderHtml` / `version` API. This is exactly how
this package's own vitest suite exercises the wasm.

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
- **The single artifact carries SWC even for `renderHtml`-only use.** One
  cdylib can't tree-shake SWC away when only `renderHtml` is called; a slim
  `renderHtml`-only artifact is a documented possible follow-up.
- **Syntax highlighting uses syntect's `fancy-regex` backend** (native zfb uses
  `oniguruma`, which can't compile to wasm). The two are byte-identical on
  zfb's fixture corpus; any grammar-level divergences are tracked in the
  crate's informational backend-divergence test.

## Artifact size

Shipping SWC in the bytes makes this a large module. The build applies a
size-optimized cargo profile (`opt-level = "z"`, LTO, one codegen unit,
`panic = "abort"`) plus `wasm-opt`, which roughly halves the raw binary. The
current build produces **~2.9 MB raw / ~1.3 MB gzipped** for the `.wasm`. The CI
`wasm-md` job prints the authoritative gzipped size on every run — treat that
as the source of truth rather than this figure, which can drift.

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
  `compile` / `renderHtml` / `version` call transparently uses the fresh
  instance. The API is stateless per call, so re-init is lossless.

  If you ever see a `ZfbMdWasmTrapError`, please report it with the input that
  triggered it — the crate is designed never to trap on structured input.
  (Fuzzing the trap surface is a documented follow-up.)

## License

MIT © Takeshi Takatsudo
