// Mirrors the JSON contracts of the `zfb-md-wasm` Rust crate (zfb#1576)
// literally -- see that crate's `src/lib.rs` rustdoc for the authoritative
// shape. Keep this file in lock-step with the crate when the crate's
// options/result shapes change.

/** `zfb_content::facade::PipelineOptions`'s `gfm` sub-object, verbatim. */
export interface GfmOptions {
  strikethrough?: boolean;
  table?: boolean;
  autolinkLiteral?: boolean;
  taskListItem?: boolean;
  footnoteDefinition?: boolean;
}

/**
 * `MarkdownFeaturesConfig` (see `crates/zfb-md-ast/src/features_config.rs`).
 * Left as an open record here -- the wasm boundary passes it through to the
 * Rust `deny_unknown_fields` deserializer verbatim, which is the
 * authoritative validator for its keys.
 */
export type MarkdownFeaturesConfig = Record<string, unknown>;

/** `zfb_content::facade::PipelineOptions`, verbatim. */
export interface PipelineOptions {
  /** A syntect theme name, or `null` for no syntax highlighting. */
  theme?: string | null;
  gfm?: GfmOptions;
  cjkFriendly?: boolean;
  hardBreaks?: boolean;
  features?: MarkdownFeaturesConfig;
}

/** `jsxRuntime` option values. Consumed only by `compile`. */
export type JsxRuntime = "preact" | "react";

/**
 * The options JSON document shared by `compile` and `renderHtml`. Every
 * field is optional; `{}` selects all defaults. Unknown fields are
 * rejected by the Rust side at both nesting levels (`deny_unknown_fields`).
 */
export interface ZfbMdWasmOptions {
  /**
   * Must end in `.md` or `.mdx`. Drives frontmatter dispatch and
   * diagnostics display. Defaults to `<anonymous>.mdx` for `compile` and
   * `<anonymous>.md` for `renderHtml`.
   */
  filename?: string;
  /** Consumed only by `compile`; `renderHtml` accepts and ignores it. */
  jsxRuntime?: JsxRuntime;
  /** Consumed only by `compile`; `renderHtml` accepts and ignores it. */
  development?: boolean;
  pipeline?: PipelineOptions;
}

/** `source` values a `Diagnostic` can carry. */
export type DiagnosticSource = "options" | "frontmatter" | "markdown" | "compile";

/**
 * One diagnostic entry. `line`/`column` are 1-based. For `"markdown"` /
 * `"frontmatter"` they point into the *original source* (frontmatter lines
 * included). For `"options"` they point into the *options JSON document*.
 * `null` when the underlying error carries no location.
 */
export interface Diagnostic {
  severity: "error";
  source: DiagnosticSource;
  message: string;
  line: number | null;
  column: number | null;
}

/** Result document of `compile`. */
export interface CompileResult {
  /** ES-module JS source on success, `null` on failure. */
  code: string | null;
  /** Parsed YAML frontmatter as JSON, `null` when absent or unextractable. */
  frontmatter: unknown;
  diagnostics: Diagnostic[];
}

/** Result document of `renderHtml`. */
export interface RenderHtmlResult {
  /** HTML fragment on success, `null` on failure. */
  html: string | null;
  frontmatter: unknown;
  diagnostics: Diagnostic[];
}

/**
 * The fixed 18-role semantic taxonomy emitted by `highlightCode`. This union
 * is mechanically checked against Rust's canonical `HiRole::FULL_NAMES` by
 * `zfb`'s role-drift guard; do not add a second unguarded role list.
 */
export type HighlightRole =
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

/** Options for direct arbitrary-code semantic class highlighting. */
export interface HighlightCodeOptions {
  /** Required syntax token, for example `"html"`, `"css"`, or `"javascript"`. */
  language: string;
  /** The only supported direct output mode. Defaults to `"class"`. */
  mode?: "class";
  /** Semantic role class prefix. Defaults to `"hi-"`. */
  classPrefix?: string;
  /** Full-name role overrides, for example `{ keyword: "text-violet-600" }`. */
  roleClasses?: Partial<Record<HighlightRole, string>>;
}

/** Sources used by direct `highlightCode` diagnostics. */
export type HighlightDiagnosticSource = "options" | "highlight" | "internal";

/** A structured direct-highlighting diagnostic. */
export interface HighlightDiagnostic {
  severity: "error" | "warning";
  source: HighlightDiagnosticSource;
  message: string;
  /** Always `null` for semantic/highlight diagnostics; JSON option parse locations are 1-based. */
  line: number | null;
  /** Always `null` for semantic/highlight diagnostics; JSON option parse locations are 1-based. */
  column: number | null;
}

/** Result document returned by {@link highlightCode}. */
export interface HighlightCodeResult {
  /** Complete semantic `<pre><code>` wrapper, or `null` for invalid options/internal errors. */
  html: string | null;
  diagnostics: HighlightDiagnostic[];
}
