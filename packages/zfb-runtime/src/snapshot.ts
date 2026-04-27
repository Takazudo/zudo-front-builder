// `@takazudo/zfb-runtime/snapshot` — TypeScript mirror of the Rust
// `ContentSnapshot` contract.
//
// The canonical shape lives in Rust at `crates/zfb-content/src/content_bridge.rs`
// (see `ContentSnapshot` and `EntrySnapshot`). The build-time pipeline
// constructs the snapshot, serializes it to JSON, and embeds it in the
// Worker bundle that miniflare loads. At Worker boot the embedded value
// is handed to [`createPageRouter`] (re-exported from the package root),
// which registers it with the `zfb/content` module so user pages calling
// `getCollection("blog")` resolve from memory rather than from the
// filesystem.
//
// Keep this in sync with the Rust struct. Field names are snake_case to
// match the JSON serialization (`module_specifier`, `rel_path`).

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
 */
export interface EntrySnapshot {
  readonly slug: string;
  readonly frontmatter: unknown;
  readonly body: string;
  readonly module_specifier: string;
  readonly rel_path: string;
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
