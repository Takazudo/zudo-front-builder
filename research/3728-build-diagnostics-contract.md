# Build diagnostic contract and emitter inventory (#3728)

Inspected base: `ae68eeffdb2d3274d5b148419fb6fa5c438ccf94`, 2026-10-05.
This is the contract handoff to #3729 (sink/CLI) and #3730 (migration warnings/docs).
Line numbers below refer to that base; symbols are the durable lookup anchors.

## Record and report

Canonical types live in `zfb_types::build_diagnostics`. `zfb-md-ast` depends on
`zfb-types` and reexports `DiagnosticSeverity` and `codes` from its existing
`diagnostics` module. No dependency on the CLI, build crate, or CSS engine is added.
The only new Cargo dependency edge is **zfb-md-ast -> zfb-types**, also recorded in
Cargo.lock. Existing zfb-types serde/serde_json/hash dependencies are unchanged.

The report constructor produces:

```json
{
  "schemaVersion": 1,
  "command": "build",
  "status": "success",
  "diagnostics": [
    {
      "code": "ZB001",
      "severity": "warning",
      "message": "broken link: #missing",
      "file": "pages/example.mdx",
      "line": 3,
      "byteColumn": 8
    }
  ]
}
```

`code`, `severity` (`info`, `warning`, `error`), and `message` are required on each
record. `sourceId`, `file`, `line`, `byteColumn` are independently optional and
omitted when unknown. `sourceId` is an opaque origin identity; it is not a file
path and must not be resolved against the working directory. Paths are producer
supplied; diagnostics do not invent a project-root path for missing sources.
Lines and UTF-8 byte columns are one-based. Path display may be lossy for paths
that cannot be represented in Unicode; the record is a reporting API, not a
filesystem round-trip API.

Status is required; there is no implicit successful/default outcome:

- `success`: all build phases and diagnostic producers finished successfully.
- `failed`: the command returned an error, including an early abort. The list is
  **potentially partial**, even if the failure happened after the last warning.
- `incomplete`: collection ended without a normal successful/failed result (for
  example, an interruption the command can handle and flush).

**Neither `failed` nor `incomplete` asserts complete diagnostic collection.**
They distinguish a known build error from an unfinished outcome, not complete
versus partial warning lists. #3729 must construct the report around the whole
command result, including configuration/setup failures and empty builds; it must
not write `success` from a drop guard or from an intermediate pipeline phase.
Hard process termination cannot guarantee a new report; an old report must never
be presented as evidence for the new invocation. File persistence/write errors
must be surfaced by #3729. No file I/O or sink lifecycle is implemented here.

`BuildDiagnostic::render()` uses `zfb warn: CODE ...` for warning severity,
`zfb info:` for informational messages, and `zfb error:` for errors. The optional
file (otherwise source ID) precedes the message. CLI status/progress formatting
remains independent. Error severity records do not themselves change build exit
semantics: the existing pipeline owns failure decisions.

## Stability and plugin compatibility

Codes describe producer-owned diagnostic classes. Keep an assigned code across
wording, path formatting, and source-position improvements. Never recycle a
retired code or infer a code from message regexes/prefixes. New classes get new
codes; consumers should decide explicitly what to do with unknown codes. Message
text is human guidance and is not a stable parsing contract. Additive optional
fields and new codes fit schema v1; removal, incompatible field/unit changes, or
renaming required fields require a new schema version.

`MarkdownDiagnostic::BrokenLink` always supplies ZB001; `Generic` has optional
`code` and defaults to ZB009 when absent/blank. Existing `warning`/`error`
constructors retain their severity behavior; built-in origins call `with_code`.
Serde support for markdown diagnostics is new (tagged `kind: brokenLink|generic`),
not a claim that the previous Rust-only enum had a JSON protocol. Existing Rust
struct literals need the new `Generic.code` / `SourceLocation.byte_column` fields;
all repository callers have been adapted. Existing `SourceLocation.col` remains
available for legacy human formatting and is **not** mapped to `byteColumn`.
Directive positions currently retain `col` and omit `byteColumn` until an emitter
has proven UTF-8 units against authored source.

Plugin `{log:{level,plugin,message}}` input still works. New `code`, `sourceId`,
`file`, `line`, `byteColumn` members are optional/defaulted. Missing/blank codes
become ZB010 regardless of prose. Missing source ID becomes `plugin:<name>`.
Legacy `column` is accepted as an unknown field and never interpreted as bytes.
Log level mapping stays `warn -> warning`, `error -> error`, everything else
`info`. Raw host stdout/stderr lines have ZB006 and source IDs
`plugin-host:stdout` / `plugin-host:stderr`; envelope/console informational logs
retain `info` severity. Plugin logging errors do not independently fail builds.

The public TypeScript `ZfbPluginDiagnosticMetadata` type is exported from both
`@takazudo/zfb/plugins` and the package root. Logger methods accept an optional
second argument. The JS host forwards only supported fields, accepts positive
u32 positions, and prevents metadata from overriding level/plugin/message.
Plugin authors should use namespaced codes such as `my-plugin/missing-asset`;
`ZB` and `ZW` are reserved for framework/engine classes. This is an ownership
convention, not a new plugin rejection policy.

## Explicit origin-to-code inventory

“Wired” below means the producer now supplies a record/code; it does not mean
build JSON collection exists. All other assignments are reserved for #3729.
Severity is warning unless stated otherwise; original severity/exit behavior
must be retained. Multiple occurrences are records, not a set.

| Code | Actual emitter(s) at base | Assignment / source facts |
| --- | --- | --- |
| ZB001 | `zfb-md-extras/src/link_validation.rs::emit_broken_link`; bundler deferred cross-file `BrokenLink` synthesis near 4720; bundler unresolved markdown URL warning near 4849 | BrokenLink accessor wired; last legacy bundler warning still to route. Path often known; positions absent. |
| ZB002 | `zfb-md-extras/src/image_dimensions.rs::emit_warning` | Wired for all image probe warnings. No message-based subclassing. |
| ZB003 | `bundler.rs::warn_dropped_plain_css_input` near 8303 | Plain CSS import deliberately omitted; source path known. |
| ZB004 | `bundler.rs::skip_dangling_symlink_or_fail` near 8265 | Dangling symlink skipped; link path known. |
| ZB005 | `jsx_pragma.rs::emit_foreign_pragma_warnings` near 208 | Foreign per-file JSX pragma; parser has file/line/column. Verify parser's column units before byte mapping. |
| ZB006 | `plugin_runner.rs::run_stderr_reader`, `handle_line` raw/unparseable stdout | Wired via `plugin_host_diagnostic`; stream source IDs, no invented file/position. |
| ZB007 | `zfb-content/src/collection.rs::walk_collection_with_cache_filter_and_options` near 544 | Unsupported extension skipped; currently `zfb warning:`. #3729 normalizes prefix; path known. |
| ZB008 | `bundler.rs` mirror fallback near 5248 and `materialise_import_meta_glob` best-effort fallback near 7829 | Same class, two original warning branches; physical/target path known. |
| ZB009 | `MarkdownDiagnostic::Generic` without code | Wired stable fallback for unclassified/third-party markdown diagnostics; preserve info/warning/error severity. |
| ZB010 | `plugin_runner.rs::handle_line` valid log envelopes | Wired fallback for plugin logger/redirected console; preserve info/warning/error severity. Explicit plugin code overrides fallback. |
| ZB011 | `zfb-md-extras/src/transclude.rs::visit_with_context`, `emit_error` | Wired missing-context warning and transclusion errors; severity remains original. |
| ZB012 | `zfb-content/src/plugins/directives.rs::visit_with_context` | Wired directive recovery/validation diagnostics, including unknown and unclosed directives; legacy col units unproven. |
| ZB013 | `zfb-content/src/mdx_jsx_emit.rs::render_jsx_attrs` invalid spread branch | Wired invalid MDX spread omitted from output. |
| ZB014 | Reserved for #3730 MDX Astro component `client:*` attributes | Future origin only; no warning emitted by #3728. |
| ZB015 | `commands/build.rs` islands scanner failure near 4524 | Scanner failure skips islands emission. |
| ZB016 | `commands/build.rs` no discovered Island targets near 4680 | Reachable page entries but no target; no invented page location. |
| ZB017 | `commands/build.rs` embedded package/esbuild extraction fallback near 4805/4820 and 6070/6085 | Explicit runtime-tool fallback; same dev helper warnings near 6750/6765 reuse code. |
| ZB018 | `commands/build.rs` `build_prerender_map` warning callback near 6850; actual read/parse branches in `render_pipeline.rs::build_prerender_map` | Cannot extract prerender data; defaults to SSG. Pass the class from that origin/callback; do not parse its string. |
| ZB019 | `commands/build.rs` SSR request-param finding near 6856 | Source-backed route contract finding; remains warning-only for build. |
| ZB020 | `commands/build.rs` empty route warnings near 6966/7215 | Empty build before or after runtime paths evaluation. |
| ZB021 | `commands/build.rs` adapter stderr loop near 7500 | Raw adapter output, one occurrence per line. |
| ZB022 | `commands/build.rs` invalid production asset output path near 7786 | Asset skipped; preserve authored/output provenance, do not relabel output as source. |
| ZB023 | `commands/build.rs::warn_deferred_dynamic` near 7822 | One skipped dynamic route per record; source path known. |
| ZB024 | `commands/build.rs` content snapshot serialization/build failures near 7942/7989 | Fallback leaves collections empty. |
| ZB025 | `commands/build.rs` render-artifact pipeline/read/metadata failures near 8059/8072/8085 | Missing page artifact; file known in per-page branches only. |
| ZB026 | `commands/build.rs::maybe_probe_content_snapshot` near 8172 | Explicit `ZFB_DEBUG_SNAPSHOT` opt-in probe failure, still a build warning when enabled. |
| ZB027 | `commands/build.rs::copy_public_dir` near 8228/8263 | Unreadable public entry or rendered-route directory collision; file only when known. |
| ZB028 | `commands/build.rs` `IslandsGlobPolicy::WarnAndSkip` near 4516/4614/4994 | **Dev-only** tolerant rebundle failures; production branches are errors, not these warnings. Reserved so shared dev human output can use identity. |
| ZB029 | `commands/build.rs` dev client-script cleanup near 6488/6518 | **Dev-only** output directory inspection/prune failure. |
| ZB030 | `commands/build.rs::build_dev_client_scripts_to_disk_with_plugin_config` collision near 6608 | **Dev-only** lenient name collision. |
| ZB031 | `bundler.rs` malformed markdown frontmatter fallback near 9357 | Currently tracing-only, not visible to CLI. Product fallback warning should be made visible/recorded by #3729, with original source path. |
| ZW001–ZW014 | `zudo-wind` typed `DiagnosticCode`; `zfb-css/src/wind_engine.rs`, `leftover_directives.rs`; `commands/build.rs` Wind source-walk/index warnings near 1154/1224/1267; human drain near 970 | Preserve existing code/severity. `CssDiagnosticOrigin.column` explicitly promises one-based bytes; map with checked conversion. Preserve candidate text in message or additive context; do not parse rendered location strings to recover provenance. |
| ZW015 | `zudo-wind/src/audit.rs` token override advisory | **Audit-only** `auditInfo`; Wind build adapter intentionally filters this severity. Do not synthesize it as a build warning. |

Inventory searches: `rg -n 'output::warn' crates/zfb/src/commands/build.rs`,
`rg -n 'zfb warn|tracing::warn|warn!' crates/zfb-build/src`, and all
`MarkdownDiagnostic::warning/error/Generic/BrokenLink` producers in content and
md-extras. No `output::warn` build.rs site is omitted above.

Internal tracing-only cache/mutex recovery, orchestrator/dev watcher upkeep,
plugin shutdown/join/unknown-reply messages, plugin-bundler tool fallback, and
persistent dev shadow lock/teardown messages are operational tracing, outside
this user-facing build warning channel. Their current severity/visibility must
not be silently converted into new user warnings. Plugin process death/hook
failure and other returned build errors remain errors; status `failed` is
required even if no structured diagnostic accompanied the returned error.
`zfb info`/success/ready/progress, build timing and debug tracing are status output,
not warning records. The product-facing markdown frontmatter fallback above is
explicitly distinguished from that internal operational tracing.

## #3729 integration obligations and remaining tests

Use explicit sink ownership across CLI, bundler, content, and async plugin
reader tasks; a CLI-only thread-local cannot capture background task output.
Collect before Markdown/Wind gates return errors, and flush after plugin readers
finish so final buffered log lines are retained. Wind currently returns only
error diagnostics in `WindDiagnosticsError` and deduplicates adapter records:
review the failure path so preceding warnings and repeated authored occurrences
are not silently lost. Build-wide deduplication is forbidden; sorting may make
ordering deterministic without deleting repeated page occurrences. Shared
pragma process-global deduplication also needs build-scope review.

Retain record identity/severity in both human and JSON paths. Do not infer codes
by wrapping the old `output::warn(msg)` with a message classifier. Add explicit
record/code arguments at the inventoried origin or its typed callback. Avoid
reinterpreting a Wind synthetic source-ID path as an actual file; inspect typed
origin provenance before flattening. Frontmatter/body-relative markdown spans
must not be claimed as authored absolute positions without mapping.

#3728 adds Rust schema/markdown/plugin backward-input tests and a cheap real
Node-host protocol test (`pnpm exec vp test run scripts/__tests__/plugin-logger-diagnostics.test.mjs`).
Manager/CI must run the affected Rust unit tests and existing plugin acceptance
fixtures; #3729 supplies successful/failed/empty-build, unwritable destination,
repeated occurrence and full-emitter sink fixtures. No warning suppression list,
per-rebuild dev JSON, or implicit warning-as-error semantics is introduced.

## Documentation handoff (external Wind ownership preserved)

#3730 should publish the code table, status meanings, optional source fields,
UTF-8 byte-column rule, plugin optional logger argument/fallbacks, and
`zfb build --warnings-json <file>` usage in EN/JA after #3729 lands. The CLI flag
is **not implemented by #3728**. Human plugin prefixes now include a code; update
examples to `zfb warn: ZB010 plugin:name: message`. General status warnings still
use their old UI helper until #3729 routes them.

No external Wind docs/generators were edited. Exact Wind documentation delta for
the manager: build JSON reuses existing ZW codes, does not include audit-only
ZW015, has required outcome status, and uses byteColumn only for proven byte
coordinates. There are no new Wind codes or changed utility behavior here.
