/**
 * Shared types for the basic-blog example.
 *
 * `BlogEntry` mirrors the shape returned by `getCollection("blog")` from
 * `zfb/content` — the `data` payload follows the frontmatter schema we
 * keep authoritative in `zfb.config.json` (see also
 * `zfb.config.future.ts` for the typed sibling that will replace the
 * JSON form once the TS config loader lands).
 */
export type BlogFrontmatter = {
  title: string;
  date: string;
  description?: string;
  tags?: string[];
};

export type BlogEntry = {
  slug: string;
  data: BlogFrontmatter;
  body: string;
};
