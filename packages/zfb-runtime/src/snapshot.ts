// `@takazudo/zfb-runtime/snapshot` — TypeScript mirror of the Rust
// `ContentSnapshot` contract.
//
// The canonical shape lives in Rust at `crates/zfb-content/src/content_bridge.rs`
// (see `ContentSnapshot` and `EntrySnapshot`). The build-time pipeline
// constructs the snapshot, serializes it to JSON, and embeds it in the
// Worker bundle that the embedded V8 host loads. At Worker boot the embedded value
// is handed to [`createPageRouter`] (exported from the server-only subpath
// `@takazudo/zfb-runtime/server`), which registers it with the `zfb/content`
// module so user pages calling
// `getCollection("blog")` resolve from memory rather than from the
// filesystem.
//
// Keep this in sync with the Rust struct. Field names are snake_case to
// match the JSON serialization (`module_specifier`, `rel_path`).

/**
 * One heading the MDX compiler allocated for an entry, in document order.
 *
 * Mirrors `crates/zfb-content/src/mdx_jsx_emit.rs::HeadingEntry`, and is
 * the same record the compiled module's `export const headings` array
 * carries — `slug` matches the rendered `<hN id="…">` because both come
 * from one slug allocation.
 */
export interface RenderHeading {
  readonly depth: number;
  readonly text: string;
  readonly slug: string;
}

/**
 * Render-artifact metadata for one content region. Mirrors
 * `crates/zfb-content/src/render_metadata.rs::RenderRegionMetadata`.
 *
 * `source_digest` is `"sha256:" + 64 hex` over the entry's RAW on-disk
 * source bytes — frontmatter included, no BOM strip, no CRLF
 * normalization. It identifies the source, not the rendered output: a
 * transcluded dependency can change what renders without changing this.
 */
export interface RenderRegionMetadata {
  readonly headings: readonly RenderHeading[];
  readonly source_digest: string;
}

/**
 * One entry in a content collection, in the shape the JS bridge sees.
 *
 * Mirrors `crates/zfb-content/src/content_bridge.rs::EntrySnapshot`.
 *
 * - `slug`: filename stem (no extension).
 * - `frontmatter`: parsed frontmatter — `null` when the source had none.
 *   Type-erased to `unknown` here; user pages narrow via the generic on
 *   `getCollection<T>()`.
 * - `body`: markdown body for `.md` / `.mdx` entries; empty string for
 *   `.tsx` entries (TSX has no separate markdown body).
 * - `module_specifier`: stable specifier addressing the compiled module
 *   (`mdx://collection/slug#hash` / `tsx://collection/slug#hash`). The
 *   bridge resolver matches either the full-with-hash form or the
 *   no-hash form `mdx://collection/slug`.
 * - `rel_path`: path relative to the collection root, normalized to
 *   forward slashes so JSON is platform-stable.
 * - `render_metadata`: present only when the build ran with
 *   `emitRenderArtifacts` on, and only for markdown entries. The Rust
 *   side skips the field entirely when the feature is off, so an
 *   unflagged build's snapshot bytes are unchanged.
 */
export interface EntrySnapshot {
  readonly slug: string;
  readonly frontmatter: unknown;
  readonly body: string;
  readonly module_specifier: string;
  readonly rel_path: string;
  readonly render_metadata?: RenderRegionMetadata;
}

/**
 * Point-in-time snapshot of every configured collection.
 *
 * Mirrors `crates/zfb-content/src/content_bridge.rs::ContentSnapshot`.
 *
 * Iteration order is documented as deterministic on the Rust side
 * (collections sorted by name, entries sorted by slug). The snapshot
 * delivered to JS preserves that order via stable JSON serialization.
 */
export interface ContentSnapshot {
  readonly collections: Readonly<Record<string, readonly EntrySnapshot[]>>;
}
