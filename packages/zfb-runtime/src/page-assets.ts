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

// ---------------------------------------------------------------------------
// Finalizer. Server-only: the router uses it, and it is exported from this
// module for tests, never from the client-safe root barrel.
// ---------------------------------------------------------------------------

const ASSET_KINDS: ReadonlySet<string> = new Set<PageAssetKind>([
  "css",
  "islands",
  "client-script",
]);

/** A prepared finalizer for one immutable asset record. */
export type HtmlAssetFinalizer = (html: string) => string;

interface Rewrite {
  readonly from: string;
  readonly to: string;
}

/**
 * Copy and freeze an asset record so later mutation of the caller's object
 * cannot change what a router applies. Throws `TypeError` on a malformed
 * record and returns `undefined` for a missing or empty one.
 */
export function snapshotPageAssets(
  manifest: PageAssetManifest | undefined,
): PageAssetManifest | undefined {
  if (manifest === undefined) return undefined;
  if (typeof manifest !== "object" || manifest === null || !Array.isArray(manifest.assets)) {
    throw new TypeError("[zfb-runtime] createPageRouter: assets must be { assets: PageAsset[] }");
  }
  if (manifest.assets.length === 0) return undefined;
  const assets = manifest.assets.map((entry: PageAsset, index: number): PageAsset => {
    if (
      typeof entry !== "object" ||
      entry === null ||
      typeof entry.kind !== "string" ||
      !ASSET_KINDS.has(entry.kind) ||
      typeof entry.stableUrl !== "string" ||
      typeof entry.url !== "string"
    ) {
      throw new TypeError(
        `[zfb-runtime] createPageRouter: assets.assets[${index}] must be { kind: "css" | "islands" | "client-script", stableUrl: string, url: string }`,
      );
    }
    return Object.freeze({ kind: entry.kind, stableUrl: entry.stableUrl, url: entry.url });
  });
  return Object.freeze({ assets: Object.freeze(assets) });
}

/**
 * Prepare the finalizer for one asset record, or `undefined` when the record
 * is missing or empty (the caller then leaves HTML byte-identical).
 */
export function createHtmlAssetFinalizer(
  manifest: PageAssetManifest | undefined,
): HtmlAssetFinalizer | undefined {
  const snapshot = snapshotPageAssets(manifest);
  if (snapshot === undefined) return undefined;

  const rewrites: Rewrite[] = [];
  for (const { stableUrl, url } of snapshot.assets) {
    if (stableUrl.length === 0) continue;
    rewrites.push({ from: stableUrl, to: url });
    // An attribute-escaped reference (`&amp;` in a query, for example) keeps
    // its escaping after the rewrite.
    const escapedFrom = escapeAttr(stableUrl);
    if (escapedFrom !== stableUrl) rewrites.push({ from: escapedFrom, to: escapeAttr(url) });
  }
  // Longest key first, like `ProductionAssetPipeline` (prod.rs). `sort` is
  // stable, so equal lengths keep record order.
  rewrites.sort((a, b) => b.from.length - a.from.length);

  const headTags = [
    ...snapshot.assets.filter((a) => a.kind === "css").map((a) => cssLinkTag(a.url)),
    ...snapshot.assets.filter((a) => a.kind === "islands").map((a) => islandModuleScriptTag(a.url)),
  ];

  return (html) => {
    let out = html;
    for (const { from, to } of rewrites) {
      if (out.includes(from)) out = boundaryReplace(out, from, to);
    }
    return injectHeadTags(out, headTags);
  };
}

/**
 * Finish managed SSR HTML with a build's finalized asset record: rewrite
 * known stable URLs to their final URLs, then add the missing CSS and islands
 * tags before the first `</head>`. Returns `html` itself when the record is
 * missing or empty. Applying it twice is a no-op.
 */
export function finalizeHtmlAssets(html: string, manifest: PageAssetManifest | undefined): string {
  const finalize = createHtmlAssetFinalizer(manifest);
  return finalize === undefined ? html : finalize(html);
}

/**
 * Port of `boundary_replace` in `crates/zfb-build/src/pipeline/prod.rs`: a
 * match is rewritten only when both neighbours are URL boundaries. JS strings
 * are UTF-16; every boundary is ASCII, so a non-ASCII neighbour (including a
 * surrogate half) is a non-boundary, exactly like a UTF-8 continuation byte
 * on the Rust side, and surrounding Unicode is copied untouched.
 */
function boundaryReplace(haystack: string, from: string, to: string): string {
  if (from.length === 0) return haystack;
  let out = "";
  let lastCopied = 0;
  let replaced = false;
  let i = haystack.indexOf(from);
  while (i !== -1) {
    const end = i + from.length;
    if (isUrlBoundary(haystack, i - 1) && isUrlBoundary(haystack, end)) {
      out += haystack.slice(lastCopied, i) + to;
      lastCopied = end;
      replaced = true;
      i = haystack.indexOf(from, end);
    } else {
      i = haystack.indexOf(from, i + 1);
    }
  }
  return replaced ? out + haystack.slice(lastCopied) : haystack;
}

/** Port of `is_url_boundary_byte` (prod.rs): start/end, or one ASCII delimiter. */
function isUrlBoundary(text: string, index: number): boolean {
  if (index < 0 || index >= text.length) return true;
  switch (text.charCodeAt(index)) {
    case 0x22: // "
    case 0x27: // '
    case 0x5c: // \
    case 0x0a: // \n
    case 0x0d: // \r
    case 0x09: // \t
    case 0x20: // space
    case 0x3c: // <
    case 0x3e: // >
    case 0x3f: // ?
    case 0x23: // #
    case 0x28: // (
    case 0x29: // )
    case 0x3d: // =
    case 0x2c: // ,
    case 0x3b: // ;
      return true;
    default:
      return false;
  }
}

/** Same bytes as `css_link_tag` in `crates/zfb-build/src/head_inject.rs`. */
function cssLinkTag(href: string): string {
  return `<link rel="stylesheet" href="${escapeAttr(href)}">`;
}

/** Same bytes as `island_module_script_tag` in `head_inject.rs`. */
function islandModuleScriptTag(src: string): string {
  return `<script type="module" src="${escapeAttr(src)}"></script>`;
}

const ATTR_ESCAPES: Readonly<Record<string, string>> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

/** Same escape set as `escape_attr` in `head_inject.rs`. */
function escapeAttr(value: string): string {
  return value.replace(/[&<>"']/g, (c) => ATTR_ESCAPES[c] ?? c);
}

/**
 * Insert `tags` before the first `</head>` (lowercase first, then any case), skipping any
 * tag whose exact text already appears before it. Without a `</head>` nothing
 * is inserted: a head or document is never synthesized.
 */
function injectHeadTags(html: string, tags: readonly string[]): string {
  if (tags.length === 0) return html;
  // Same lookup order as `find_close_head` in head_inject.rs: a lowercase
  // `</head>` first, then any case, so SSR and SSG pick the same position.
  const lowerAt = html.indexOf("</head>");
  const closeAt = lowerAt !== -1 ? lowerAt : html.search(/<\/head>/i);
  if (closeAt === -1) return html;
  const before = html.slice(0, closeAt);
  const missing = tags.filter((tag) => !before.includes(tag));
  if (missing.length === 0) return html;
  return before + missing.join("") + html.slice(closeAt);
}
