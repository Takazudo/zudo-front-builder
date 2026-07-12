# zfb-md-wasm

zfb's md/mdx → JS/HTML conversion pipeline compiled to WebAssembly
(`wasm32-unknown-unknown`) for browser-side dynamic conversion — the primary
use case is CMS live preview with **parity** to zfb's production output
(epic zfb#1572, crate created in zfb#1576).

Two API tiers live in one cdylib (the tier split buys a smaller runtime
working set, not a smaller download — SWC is in the bytes either way; a slim
renderHtml-only artifact is a documented possible follow-up):

| Export (JS name) | Pipeline | Returns (JSON string) |
|---|---|---|
| `compile(source, optionsJson)` | mdx → JSX → SWC → ES-module JS | `{ code, frontmatter, diagnostics }` |
| `renderHtml(source, optionsJson)` | md → mdast → visitors → hast → HTML | `{ html, frontmatter, diagnostics }` |
| `version()` | — | crate version string |

The whole boundary is JSON-in/JSON-out strings; the authoritative shape
documentation is the crate rustdoc (`src/lib.rs`) plus
`zfb_content::facade::PipelineOptions` for the `pipeline` sub-object.

## Options JSON (shared by both entry points)

```json
{
  "filename": "posts/hello.mdx",
  "jsxRuntime": "preact",
  "development": false,
  "pipeline": {
    "theme": null,
    "gfm": { "strikethrough": true, "table": true, "autolinkLiteral": false,
             "taskListItem": false, "footnoteDefinition": false },
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
*later* stage fails. `diagnostics` is empty on success, otherwise:

```json
{ "severity": "error", "source": "markdown",
  "message": "Expected a closing tag for `<Card>` (1:1) (markdown-rs:end-tag-mismatch)",
  "line": 7, "column": 1 }
```

- `source` ∈ `"options"` | `"frontmatter"` | `"markdown"` | `"compile"`.
- `line`/`column` are 1-based. For `"markdown"`/`"frontmatter"` they point
  into the **original source** (positions markdown-rs reports against the
  frontmatter-stripped body are shifted back; YAML positions are shifted
  past the opening `---`). For `"options"` they point into the **options
  JSON document**. `null` when the underlying error carries no location.
- Caveat: the human-readable `message` text may embed positions in the
  coordinate space the underlying library used (body-relative for
  markdown-rs, YAML-relative for serde_yaml). The **structured fields** are
  the contract; the message is display-only.

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

## Wasm-target blockers and their resolutions

Every blocker met on the way to `wasm32-unknown-unknown`, and what resolved
it (details: `SPIKE-FINDINGS.md` in this directory for the swc half):

| Blocker | Resolution |
|---|---|
| `onig_sys` (C oniguruma) via syntect's default `regex-onig` backend | zfb-content grew `syntect-onig`/`syntect-fancy` backend features (zfb#1573); this crate selects `syntect-fancy` (pure Rust). Native builds keep onig — when feature unification enables both, syntect gives regex-onig precedence, so native output stays byte-identical. |
| `zfb-render → zfb-content` edge re-unified `syntect-onig` into every downstream graph (dependency default features cannot be subtracted) | zfb#1576 made that edge `default-features = false` and added forwarding features `syntect-onig` (in zfb-render's default set — native builds unchanged) / `syntect-fancy` to zfb-render. Consequence: syntect does not compile with NO backend selected, so health.yml's `cargo test --no-default-features -p zfb-render` step now pins `--features syntect-onig` (that step tests the V8-off toggle, not the backend choice). |
| `deno_core` + `tokio` (embedded V8 host) | Behind zfb-render's default-on `embed_v8` feature; this crate depends on zfb-render with `default-features = false` (a CI-tested configuration — the `build (no-v8)` job). |
| swc_core on wasm (the historical unknown) | The zfb#1575 spike proved swc_core `=64.0.0` with zfb-render's exact feature set compiles AND executes on `wasm32-unknown-unknown` with zero special configuration — no rustflags, no `.cargo/config.toml`, no version pins. See `SPIKE-FINDINGS.md`. |
| `getrandom` (usual wasm suspect) | **Not in the graph** (all three majors in the workspace lock are pulled only by native-only tooling; wasm-bindgen adds none). Do not add workarounds preemptively — if a future dep drags getrandom ≥0.3 in, the known-good target-scoped cfg is documented in `SPIKE-FINDINGS.md`. |
| `clippy::needless_return` in `zfb-types::module_workers` fired only under the wasm cfg (a `return` whose following `#[cfg(windows)]` block compiles away on non-unix non-windows targets) | Restructured to `let … else` so the diverging `return` is required on every target (zfb#1576). |
| `std::time::Instant` (panics at runtime on wasm32 if called) | Not called on the exercised parse→transform→codegen path (execution-proven by the spike). Compile-green everywhere; unexercised swc branches remain a rule-4 residual risk. |

The graph stays clean by assertion, not eyeballing:
`scripts/assert-zfb-md-wasm-graph.sh` greps
`cargo tree --target wasm32-unknown-unknown -p zfb-md-wasm -e normal` for
`onig_sys`/`deno_core`/`tokio` (CI wiring is the wave-4 sub-issue zfb#1579).

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

- Fuzzing the `compile`/`renderHtml` boundary for panic-freedom (trap rule 4).
- Optional `console_error_panic_hook` (or equivalent) behind a feature for
  debuggable trap messages during development.
- A slim renderHtml-only artifact without SWC in the bytes.
