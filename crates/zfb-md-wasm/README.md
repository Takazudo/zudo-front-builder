# zfb-md-wasm

zfb's md/mdx → JS/HTML conversion pipeline compiled to WebAssembly
(`wasm32-unknown-unknown`) for browser-side dynamic conversion — the primary
use case is CMS live preview with **parity** to zfb's production output
(epic zfb#1572, crate created in zfb#1576).

The crate builds one full compatibility surface and three additive capability
surfaces. The npm package publishes each surface as an entry with its own
wasm resource pair; choose the smallest entry that covers the calls you make:

| Export (JS name)                   | Pipeline                               | Returns (JSON string)                  |
| ---------------------------------- | -------------------------------------- | -------------------------------------- |
| `compile(source, optionsJson)`     | mdx → JSX → SWC → ES-module JS         | `{ code, frontmatter, diagnostics }`   |
| `renderHtml(source, optionsJson)`  | md → mdast → visitors → hast → HTML    | `{ html, frontmatter, diagnostics }`   |
| `highlightCode(code, optionsJson)` | arbitrary source → semantic class HTML | `{ html, diagnostics }`                |
| `version()`                        | —                                      | release-stamped package version string |

The whole boundary is JSON-in/JSON-out strings; the authoritative shape
documentation is the crate rustdoc (`src/lib.rs`) plus
`zfb_content::facade::PipelineOptions` for the `pipeline` sub-object.
Published artifacts stamp `version()` with the package semver during release;
local development builds fall back to the Rust manifest version placeholder.

## Entry selection and isolated resources

The root entry (`.`) is the complete current API and is the only entry that
exports `compile`. The existing `./highlight` entry remains compatible and is
the direct-highlighting entry. The additive `./render` and `./parse` entries
are SWC-free isolated graphs: they omit `swc_core` and `zfb-render`, while
intentionally retaining `zfb-content` and its `syntect-fancy` backend. Parse
is not syntect-free.

| Entry         | gzip-9 wasm (2.14.3) | Runtime values                                                                                                                                                                                                                  | Type surface                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| ------------- | -------------------: | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.` | 1,514,540 B | `init`, `compile`, `renderHtml`, `parseToAst`, `highlightCode`, `version`, `__forceTrapForTests`, `__getTrapRecoveryStateForTests`, `toMdastRoot`, `ZfbMdWasmTrapError`, `ZfbMdWasmTrapRecoveryLimitError`, `MdastAdapterError` | Full current compile, render, parse/raw-mdast, and highlight types |
| `./highlight` | 817,922 B | `init`, `highlightCode`, `version`, `__forceTrapForTests`, `__getTrapRecoveryStateForTests`, `ZfbMdWasmTrapError`, `ZfbMdWasmTrapRecoveryLimitError` | `HighlightRole`, `HighlightCodeOptions`, `HighlightCodeResult`, `HighlightDiagnostic`, `HighlightDiagnosticSource` |
| `./render` | 1,088,857 B | `init`, `renderHtml`, `version`, `ZfbMdWasmTrapError`, `ZfbMdWasmTrapRecoveryLimitError`, `__forceTrapForTests`, `__getTrapRecoveryStateForTests` | `RenderHtmlResult`, `Diagnostic`, `DiagnosticSource`, `ZfbMdWasmOptions`, `PipelineOptions`, `GfmOptions`, `CodeHighlightMode`, `CodeHighlightOptions`, `MarkdownFeaturesConfig`, `JsxRuntime`, `HighlightRole` |
| `./parse` | 281,395 B | `init`, `parseToAst`, `toMdastRoot`, `MdastAdapterError`, `version`, `ZfbMdWasmTrapError`, `ZfbMdWasmTrapRecoveryLimitError`, `__forceTrapForTests`, `__getTrapRecoveryStateForTests` | `ParseToAstResult`, `ParseToAstOptions`, `ParseDialect`, `FrontmatterPolicy`, `ParsePipelineOptions`, `Diagnostic`, `DiagnosticSource`, `AstPoint`, `AstPosition`, `RawMdastData`, `MarkdownRsStop`, `MdastNode`, `MdastRoot`, `UnknownMdastNode`, `Root`, `Paragraph`, `Heading`, `ThematicBreak`, `Blockquote`, `List`, `ListItem`, `Html`, `Code`, `Definition`, `Text`, `DirectiveNodeBase`, `ContainerDirective`, `LeafDirective`, `TextDirective`, `Emphasis`, `Strong`, `InlineCode`, `Break`, `Link`, `Image`, `ReferenceKind`, `LinkReference`, `ImageReference`, `FootnoteDefinition`, `FootnoteReference`, `TableAlign`, `Table`, `TableRow`, `TableCell`, `Delete`, `Yaml`, `MdxFlowExpression`, `MdxTextExpression`, `MdxJsxFlowElement`, `MdxJsxTextElement`, `MdxJsxAttributeContent`, `MdxJsxAttribute`, `MdxJsxAttributeValueExpression`, `MdxJsxExpressionAttribute` |

The focused entries have private, non-interchangeable resource pairs:

```text
wasm-render/zfb_md_wasm_render_glue.zfb-resource.mjs
wasm-render/zfb_md_wasm_render_bg.wasm
wasm-parse/zfb_md_wasm_parse_glue.zfb-resource.mjs
wasm-parse/zfb_md_wasm_parse_bg.wasm
```

Their declaration sidecars are also private to the matching directory. Each
entry creates one independent `createWasmApi` state: compiled module, wasm
instance, generation, retry state, and terminal state are not shared. Importing
multiple entries intentionally loads independent resource pairs and instances;
this is useful when both calls are required, but it costs both downloads.

Root imports and `./highlight` imports need no migration. Replace a root import
that only calls `renderHtml` or `parseToAst` with the matching focused entry:

```ts
// Node and browser-aware bundlers: direct entry selection.
import { renderHtml } from "@takazudo/zfb-md-wasm/render";
import { parseToAst, toMdastRoot } from "@takazudo/zfb-md-wasm/parse";
```

For a browser user action, keep those imports lazy so the matching pair is
fetched only when needed:

```ts
button.addEventListener("click", async () => {
  const { renderHtml } = await import("@takazudo/zfb-md-wasm/render");
  const { html } = await renderHtml(source, { filename: "preview.md" });
  preview.innerHTML = html ?? "";
});

parseButton.addEventListener("click", async () => {
  const { parseToAst, toMdastRoot } = await import("@takazudo/zfb-md-wasm/parse");
  const parsed = await parseToAst(source, { filename: "preview.md" });
  const root = parsed.ast === null ? null : toMdastRoot(parsed.ast);
  inspect(root);
});
```

The browser-aware conditional export supplies static `?url` edges to only the
entry's own pair; the direct Node path uses relative files (and fetch outside
Node). `renderHtml` is not a sanitizer: raw HTML remains untrusted. Parsed
MDX JSX, expression, and ESM-shaped nodes are inert data. A controlled
consumer may interpret the parsed data in an AST-to-React renderer, but no slim
entry evaluates author JavaScript. `compile` remains root-only and returns module
source that requires host evaluation plus a JSX runtime/components boundary.

## Options JSON (root and `./render`)

```json
{
  "filename": "posts/hello.mdx",
  "jsxRuntime": "preact",
  "development": false,
  "pipeline": {
    "theme": null,
    "gfm": {
      "strikethrough": true,
      "table": true,
      "autolinkLiteral": true,
      "taskListItem": false,
      "footnoteDefinition": false
    },
    "cjkFriendly": true,
    "hardBreaks": false,
    "features": {}
  }
}
```

- Every field is optional; `"{}"` selects all defaults. Unknown fields are
  rejected at both nesting levels (`deny_unknown_fields`).
- `filename` must end in `.md`/`.mdx` (frontmatter dispatch + diagnostics
  display). Defaults: `<anonymous>.mdx` for `compile`, `<anonymous>.md` for
  `renderHtml`.
- `jsxRuntime` (`"preact"` | `"react"`, default `"preact"`) and
  `development` (default `false`) are consumed only by `compile`;
  `renderHtml` accepts and ignores them so one options document can serve
  both tiers.
- `pipeline` is `zfb_content::facade::PipelineOptions` verbatim (zfb#1574):
  `theme` is a **syntect** theme name, `features` is
  `MarkdownFeaturesConfig` verbatim. Config arrives **already resolved** —
  `zfb.config.ts` evaluation needs V8 and stays build-side.

## Result JSON

`code` / `html` is a string on success and `null` on failure. `frontmatter`
is the parsed YAML frontmatter as JSON (`null` when the source has none);
frontmatter values that were successfully extracted are returned even when a
_later_ stage fails. `diagnostics` is empty on success, otherwise:

```json
{
  "severity": "error",
  "source": "markdown",
  "message": "Expected a closing tag for `<Card>` (1:1) (markdown-rs:end-tag-mismatch)",
  "line": 7,
  "column": 1
}
```

- `source` ∈ `"options"` | `"frontmatter"` | `"markdown"` | `"compile"`.
- `line`/`column` are 1-based. For `"markdown"`/`"frontmatter"` they point
  into the **original source** (positions markdown-rs reports against the
  frontmatter-stripped body are shifted back; YAML positions are shifted
  past the opening `---`). Markdown diagnostic columns use JavaScript UTF-16
  code units, including surrogate pairs, matching `parseToAst` positions and
  `String.prototype.slice`; they are not UTF-8 bytes or grapheme clusters.
  For `"options"` they point into the **options JSON document**. `null` when
  the underlying error carries no location, or when an upstream location is
  malformed or outside the parsed body.
- `message` is opaque display text. Do not parse or rewrite it: it may embed
  positions in the coordinate space the underlying library used (body-relative
  for markdown-rs, YAML-relative for serde_yaml). The structured `line` and
  `column` fields are the sole supported locations.

## Direct semantic code highlighting

`highlightCode` is a third, closed class-mode API for syntax-highlighting an
arbitrary source string without wrapping it in a Markdown fence. At the raw
wasm boundary it receives an options JSON string; the published npm root
exports the typed convenience form:

```ts
import {
  highlightCode,
  type HighlightCodeOptions,
  type HighlightCodeResult,
} from "@takazudo/zfb-md-wasm";

const result: HighlightCodeResult = await highlightCode("const answer = 42;", {
  language: "javascript",
  mode: "class", // optional; this is the only supported mode
  classPrefix: "hi-", // optional; the default
  roleClasses: { keyword: "text-violet-600 dark:text-violet-400" },
} satisfies HighlightCodeOptions);
```

`language` is required. `mode` can only be `"class"`; `classPrefix` changes
both the root (`${classPrefix}root`) and default token classes. The output is
escaped semantic markup, not inline-colour/Shiki output:

```html
<pre class="hi-root"><code><span class="line"><span class="hi-kw">const</span> …</span></code></pre>
```

The fixed taxonomy uses full names as `roleClasses` override keys and the
following default class suffixes: `escape` → `hi-esc`, `operator` → `hi-op`,
`comment` → `hi-com`, `string` → `hi-str`, `number` → `hi-num`, `constant` →
`hi-const`, `keyword` → `hi-kw`, `function` → `hi-fn`, `type` → `hi-ty`,
`namespace` → `hi-ns`, `property` → `hi-prop`, `variable` → `hi-var`, `tag` →
`hi-tag`, `attribute` → `hi-attr`, `punctuation` → `hi-punct`, `inserted` →
`hi-ins`, `deleted` → `hi-del`, and `heading` → `hi-hd`. An override replaces
that role's default class; use `keyword`, not the suffix `kw`, as the key.

Invalid JSON/options (including a missing/empty language, another `mode`, a
bad prefix, unknown role key, or extra fields) return `{ html: null,
diagnostics: [{ severity: "error", source: "options", … }] }`. An unknown
non-empty language is different: it succeeds with escaped fallback wrapper
markup and a `{ severity: "warning", source: "highlight", line: null,
column: null }` diagnostic. Incomplete editor input is valid input and may
highlight without a diagnostic.

## Error / trap / re-init contract (correctness-critical)

The npm wrapper (zfb#1577) implements auto-reinit-on-trap against exactly
this contract:

1. **Expected failures never trap.** Markdown parse errors, malformed
   options JSON, unknown syntect theme names, YAML frontmatter errors, and
   non-markdown filenames all return a result document with `code`/`html:
null` and a structured diagnostic. The call path was audited for
   user-input-reachable `unwrap`/`expect` (zfb#1576) — notably
   `facade::build_pipeline` was made fallible because theme-name validation
   (zfb#1067/zfb#1070) fires at pipeline construction with names straight
   from options JSON; the facade previously `.expect`ed that path away.
2. **A panic is a bug, and it poisons the instance.** On
   `wasm32-unknown-unknown` a Rust panic becomes a wasm trap;
   `catch_unwind` is not reliable recovery, and after a trap the instance's
   memory/globals must be assumed corrupt. The host wrapper MUST drop the
   instance and re-instantiate the module on any trap.
3. **Re-init is lossless.** The API is stateless across calls — each call
   parses options, builds a fresh pipeline, and returns; no caches or
   globals survive between calls — so re-instantiation loses nothing but
   time.
4. **Residual risk.** Third-party internals (markdown-rs, swc, syntect,
   fancy-regex) can still panic on pathological input (e.g. deep nesting /
   recursion limits); that risk is exactly what rule 2 covers. Fuzzing the
   boundary is a noted follow-up, out of scope for zfb#1576.

The npm wrapper applies this rule to all three calls, including
`highlightCode`: it throws `ZfbMdWasmTrapError` for the trapped call, eagerly
starts one fresh instance from its cached compiled `WebAssembly.Module`, and
the next public call uses that replacement. Browser recovery deliberately
loads a fresh glue module URL with `?zfbMdWasmGen=N`; the `.wasm` bytes are not
downloaded or compiled again. Repeated traps are bounded (16 recoveries) to
avoid unbounded browser module records.

## Browser resource delivery

The published package's browser exports statically declare exactly two runtime
resources per entry. Root keeps `wasm/zfb_md_wasm_glue.zfb-resource.mjs` and
`wasm/zfb_md_wasm_bg.wasm`; highlight keeps its existing
`wasm-highlight/zfb_md_wasm_highlight_glue.zfb-resource.mjs` and
`wasm-highlight/zfb_md_wasm_highlight_bg.wasm`. The focused entries use the
private pairs listed above. A zfb production build emits each as a hashed
sibling asset. Put an entry behind a user-triggered dynamic import when lazy
loading matters: only that entry's glue and wasm are then fetched on first use.

The static server must return the emitted glue as JavaScript
(`application/javascript`) and the wasm as `application/wasm`, with ordinary
HTTP success responses. Do not copy those resources manually or import the
package source path; consume the packed browser entry so the generated URLs
stay correct under a hashed island bundle.

## Shipped artifact sizes and locked ceilings

### Maintainer repair workflow

These guarded size numbers use `crates/zfb-md-wasm/shipped-sizes.json` as their
source of truth. After an intentional artifact change, use this verified
three-step repair sequence:

1. Re-run the four-artifact build and capture its summary:
   `BUILD_LOG=/tmp/zfb-md-wasm-build.log; node crates/zfb-md-wasm/npm/scripts/build.mjs 2>&1 | tee "$BUILD_LOG"`.
2. Run `node scripts/assert-zfb-md-wasm-budgets.mjs --build-log "$BUILD_LOG" --dist crates/zfb-md-wasm/npm/dist --update-manifest`.
3. Run `node scripts/assert-md-wasm-size-docs.mjs --fix`, then `pnpm format:mdx`.

These are the shipped **2.14.3** artifact rows — optimized final wasm after
wasm-bindgen and wasm-opt, Node `gzipSync(..., { level: 9 })`, and glue
bytes/gzip:

| Entry/graph |  final wasm |      gzip-9 |     glue | glue gzip-9 |
| ----------- | ----------: | ----------: | -------: | ----------: |
| root (full) | 3,394,144 B | 1,514,540 B | 14,998 B | 4,199 B |
| highlight | 1,539,186 B | 817,922 B | 8,758 B | 2,637 B |
| render | 2,189,671 B | 1,088,857 B | 8,772 B | 2,661 B |
| parse | 693,479 B | 281,395 B | 11,159 B | 3,797 B |

The #2447 decision snapshot measured the split package at 3,638,607 B versus
the root-plus-highlight package at 2,314,818 B. Locked gzip-9 ceilings are
root 1,600,000 B, highlight 880,000 B, render 1,100,000 B, and parse
325,000 B; the complete packed tarball ceiling is 3,900,000 B. All four ship
inside their ceilings, with 85,460 B (root), 62,078 B (highlight), 11,143 B
(render), and 43,605 B (parse) of headroom. These are 2.14.3 measurements, not
permanent promises — re-measure against the version you actually install. The
four-step clean production reference ceiling is 210 seconds, with the #2447
selected median at 155.015 s [153.496, 165.977].

Gating `swc_core` out of the highlight graph (#2449/#2450) was a
**provability win, not a size win**. The #2447 SWC-retaining baseline was
1,484,705 B raw and 767,009 B gzip-9; the #2450 result was 7,965 B smaller
raw and 8,765 B smaller gzip-9 (758,244 B) — wasm-opt was already
dead-stripping the unreachable `swc_core`, and #2450's exact-parity and
no-`swc_core` assertions turned that emergent property into a guaranteed one.
The delta that matters to a highlight-only consumer is root versus
highlight: the highlight artifact is 1,854,958 B smaller raw and 696,618 B
smaller gzip-9, landing at about 45% of root's raw bytes and 54% of its
gzipped bytes.

Unlike 2.14.2, the 2.14.3 root, highlight, and render final-wasm rows are
above their historical #2447 candidate measurements; parse remains below.
Those candidate measurements are diagnostic baselines, not ceilings. The
enforced gzip-9 ceilings and current headroom are listed above, and the glue
comparison to #2447 is unchanged from 2.14.2.

## Wasm-target blockers and their resolutions

Every blocker met on the way to `wasm32-unknown-unknown`, and what resolved
it (details: `SPIKE-FINDINGS.md` in this directory for the swc half):

| Blocker                                                                                                                                                                                   | Resolution                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `onig_sys` (C oniguruma) via syntect's default `regex-onig` backend                                                                                                                       | zfb-content grew `syntect-onig`/`syntect-fancy` backend features (zfb#1573); this crate selects `syntect-fancy` (pure Rust). Native builds keep onig — when feature unification enables both, syntect gives regex-onig precedence, so native output stays byte-identical.                                                                                                                                                      |
| `zfb-render → zfb-content` edge re-unified `syntect-onig` into every downstream graph (dependency default features cannot be subtracted)                                                  | zfb#1576 made that edge `default-features = false` and added forwarding features `syntect-onig` (in zfb-render's default set — native builds unchanged) / `syntect-fancy` to zfb-render. Consequence: syntect does not compile with NO backend selected, so health.yml's `cargo test --no-default-features -p zfb-render` step now pins `--features syntect-onig` (that step tests the V8-off toggle, not the backend choice). |
| `deno_core` + `tokio` (embedded V8 host)                                                                                                                                                  | Behind zfb-render's default-on `embed_v8` feature; this crate depends on zfb-render with `default-features = false` (a CI-tested configuration — the `build (no-v8)` job).                                                                                                                                                                                                                                                     |
| swc_core on wasm (the historical unknown)                                                                                                                                                 | The zfb#1575 spike proved swc_core `=64.0.0` with zfb-render's exact feature set compiles AND executes on `wasm32-unknown-unknown` with zero special configuration — no rustflags, no `.cargo/config.toml`, no version pins. See `SPIKE-FINDINGS.md`.                                                                                                                                                                          |
| `getrandom` (usual wasm suspect)                                                                                                                                                          | **Not in the graph** (all three majors in the workspace lock are pulled only by native-only tooling; wasm-bindgen adds none). Do not add workarounds preemptively — if a future dep drags getrandom ≥0.3 in, the known-good target-scoped cfg is documented in `SPIKE-FINDINGS.md`.                                                                                                                                            |
| `clippy::needless_return` in `zfb-types::module_workers` fired only under the wasm cfg (a `return` whose following `#[cfg(windows)]` block compiles away on non-unix non-windows targets) | Restructured to `let … else` so the diverging `return` is required on every target (zfb#1576).                                                                                                                                                                                                                                                                                                                                 |
| `std::time::Instant` (panics at runtime on wasm32 if called)                                                                                                                              | Not called on the exercised parse→transform→codegen path (execution-proven by the spike). Compile-green everywhere; unexercised swc branches remain a rule-4 residual risk.                                                                                                                                                                                                                                                    |

The graph stays clean by assertion, not eyeballing:
`scripts/assert-zfb-md-wasm-graph.sh` greps
`cargo tree --target wasm32-unknown-unknown -p zfb-md-wasm -e normal` for
`onig_sys`/`deno_core`/`tokio`. It runs in CI on every PR via the `wasm-md`
job in `.github/workflows/health.yml` (added in zfb#1579).

## Parity notes

- `compile` output is the production emitter chain verbatim:
  `zfb_content::facade::render_mdx_jsx_module` (mdx → JSX text) →
  `zfb_render::SwcPipeline` (JSX → JS) — the same "one place where JSX
  becomes JS" the zfb binary uses, which is the whole justification for
  this package vs `@mdx-js/mdx` in the browser.
- The emitted module imports `Fragment` from `"react/jsx-runtime"`
  regardless of `jsxRuntime` — that is the production emitter's own shape
  (`mdx_jsx_emit.rs` writes it verbatim; zfb's bundler resolves it via
  aliasing). Browser hosts consuming the emitted JS directly need an import
  map / bundler alias for it under preact — an npm-wrapper (zfb#1577)
  concern.
- The fs-bound feature plugins (`transclude`, `imageDimensions`,
  `linkValidation`) are registered but **inert**: the facade never arms
  build-context roots, matching the existing MDX loader path.

## Verification (zfb#1576 acceptance)

```sh
rustup target add wasm32-unknown-unknown   # job-local; rust-toolchain.toml stays untouched (baked epic decision)
cargo check  --target wasm32-unknown-unknown -p zfb-md-wasm
cargo clippy --target wasm32-unknown-unknown -p zfb-md-wasm -- -D warnings
scripts/assert-zfb-md-wasm-graph.sh
cargo test -p zfb-md-wasm                  # native rlib tests: both tiers + diagnostics contract
```

## Follow-ups (documented, out of scope here)

- Fuzzing the focused and compile boundaries for panic-freedom (trap rule 4).
- Optional `console_error_panic_hook` (or equivalent) behind a feature for
  debuggable trap messages during development.
