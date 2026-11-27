// Shared contract for finalized production assets in request-time SSR (#4887).
//
// `zfb build` embeds this record into the SSR-only runtime bundle and passes it
// to `createPageRouter({ assets })`. The router uses it to finish managed HTML
// pages: known stable URLs are replaced with their final URLs, and the CSS and
// islands entries are added to `<head>` when missing. Client-script entries
// only resolve explicit references; they are never added automatically.
//
// URLs are the exact public strings the production asset pipeline used
// (`stableUrl` already carries the configured base). They are raw strings:
// no HTML escaping and no percent-encoding. Escaping happens only when a tag
// is written.

/** Asset category of one finalized asset. Serialized by the Rust build. */
export type PageAssetKind = "css" | "islands" | "client-script";

/** One shipped asset: the stable public URL and its final public URL. */
export interface PageAsset {
  readonly kind: PageAssetKind;
  readonly stableUrl: string;
  readonly url: string;
}

/**
 * The finalized asset record for one build. Entries are sorted (CSS, islands,
 * client scripts, then `stableUrl`) and unique by `stableUrl`.
 */
export interface PageAssetManifest {
  readonly assets: readonly PageAsset[];
}
