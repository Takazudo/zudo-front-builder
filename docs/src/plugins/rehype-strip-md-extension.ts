import type { Root, Element } from "hast";
import { visit } from "unist-util-visit";
import { isExternal } from "./url-utils";

export interface RehypeStripMdExtensionOptions {
  /** Whether to append a trailing slash after stripping the extension. Mirrors the site-wide trailingSlash setting. */
  trailingSlash: boolean;
}

/**
 * Rehype plugin factory that strips .md/.mdx extensions from relative link hrefs.
 * When `trailingSlash` is true, appends "/" after stripping the extension (and also
 * adds a trailing slash to already-extensionless relative links that lack one).
 * When `trailingSlash` is false, strips the extension only — no slash is appended —
 * so in-content links match the slash-stripped nav/source-map URLs.
 *
 * Markdown authors often write links like `[Other doc](./other-doc.md)`.
 * In Astro's static output, the correct URL is `/docs/other-doc` (no extension, no slash)
 * when trailingSlash is false, or `/docs/other-doc/` when trailingSlash is true.
 */
export function rehypeStripMdExtension(options: RehypeStripMdExtensionOptions) {
  const { trailingSlash } = options;
  return (tree: Root) => {
    visit(tree, "element", (node: Element) => {
      if (node.tagName !== "a") return;
      const href = node.properties?.href;
      if (typeof href !== "string") return;

      // Only process relative links (not http://, https://, mailto:, #, etc.)
      if (isExternal(href) || href.startsWith("#")) return;

      let newHref = href;

      // Strip .md or .mdx extension (with optional hash fragment).
      // Append trailing slash only when trailingSlash is true.
      newHref = newHref.replace(/\.mdx?(#.*)?$/, (_match, hash) => {
        if (hash) {
          return trailingSlash ? "/" + hash : hash;
        }
        return trailingSlash ? "/" : "";
      });

      // For relative links where Astro already stripped .md:
      // add trailing slash if missing and the last segment has no file extension.
      // Only applicable when trailingSlash is true — its sole purpose is appending slashes.
      if (
        trailingSlash &&
        newHref === href &&
        (newHref.startsWith("./") || newHref.startsWith("../"))
      ) {
        // Split off query string and hash fragment
        const qIdx = newHref.indexOf("?");
        const hIdx = newHref.indexOf("#");
        const suffixIdx = qIdx >= 0 ? qIdx : hIdx >= 0 ? hIdx : -1;
        const path = suffixIdx >= 0 ? newHref.slice(0, suffixIdx) : newHref;
        const suffix = suffixIdx >= 0 ? newHref.slice(suffixIdx) : "";

        if (!path.endsWith("/")) {
          const lastSegment = path.split("/").pop() || "";
          // Only add slash if there's no file extension (like .png, .pdf, etc.)
          if (!/\.[a-zA-Z]\w*$/.test(lastSegment)) {
            newHref = path + "/" + suffix;
          }
        }
      }

      if (newHref !== href) {
        node.properties.href = newHref;
      }
    });
  };
}
