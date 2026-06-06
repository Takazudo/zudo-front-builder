# zfb-md-ast

Shared AST types, visitor traits, and visitor-contract context for the zfb markdown/MDX pipeline.

This crate is the dependency boundary that lets `zfb-md-extras` (and other downstream plugin crates) implement visitors without depending on `zfb-content`. It carries the visitor contract end-to-end: the AST type, the visitor traits, and the orchestration context types those traits reference.

## Public API

- **`HastNode`** — lightweight HTML AST node (`Root`, `Element`, `Text`, `Raw`, `JsxRaw`, `Comment`). Plugins operate on this in memory; the serializer turns it into an HTML string later. `JsxRaw` and `Raw` are kept distinct so the JSX-emit path can pick the right embedding strategy without parsing the payload.
- **`MdastVisitor`** / **`HastVisitor`** — in-place mutation traits for mdast and hast trees. The pipeline does not auto-recurse; each visitor decides its own traversal strategy.
  - `visit_with_context(&mut node, &mut BuildContext)` — wave-6 seam for visitors that need source path, project root, or diagnostics. Defaults to calling `visit`.
  - `HastVisitor::reset()` — called between documents to clear per-document state (e.g. duplicate-slug counters). Default is a no-op.
- **`BuildContext<'a>`** — per-document context threaded into pipeline visitors by `Pipeline::run_with_context`. Carries `source_path`, `project_root`, `public_dir`, an optional `HeadingRegistry` reference, and an optional `DiagnosticsSink` reference.
- **`diagnostics`** — `MarkdownDiagnostic` enum (`BrokenLink`, `Generic`), `DiagnosticSeverity`, `SourceLocation`, `DiagnosticsSink` trait, and `CollectingSink` (Vec-backed sink for tests and batch processing).
- **`directives`** — `DirectiveDef`, `DirectiveKind`, `AttrSchema`, `AttrType`, `ValidatedAttrValue`, `AttrValidationResult`, `DirectiveDiagnostic`. Shared between `zfb-content` and `zfb-md-extras` so the latter can produce `Vec<DirectiveDef>` presets without a cycle.
- **`features_config`** — `MarkdownFeaturesConfig`, `FeatureToggle`, `FeatureOptions`, and supporting types for per-feature markdown pipeline configuration. Re-exported by `zfb` from its `config` module for backwards compatibility.
- **`heading_registry`** — `HeadingRegistry` (build-scoped heading-ID registry; used by `HeadingLinksPlugin` and link-validation plugins).
- **`hast_text::extract_text`** — utility that extracts plain text content from a `HastNode` subtree.

## Why this crate exists

`zfb-md-extras` implements visitor presets (`Vec<DirectiveDef>`, custom plugins) that `zfb-content` consumes. Putting the visitor trait and directive types inside `zfb-content` would create a dependency cycle: `zfb-content` → `zfb-md-extras` → `zfb-content`. `zfb-md-ast` is the cycle-breaking leaf — it has no dependency on either, and both crates can depend on it freely. `zfb-content` re-exports all types from here for backwards-compatible consumer import paths.

## Tests

```sh
cargo test -p zfb-md-ast
```

- `src/diagnostics.rs` — `CollectingSink` receives emitted diagnostics; `BrokenLink` severity; `DiagnosticSeverity` ordering.
- In-crate `#[cfg(test)]` blocks cover `HastNode`, `BuildContext`, and `directives` types inline with their implementations.
