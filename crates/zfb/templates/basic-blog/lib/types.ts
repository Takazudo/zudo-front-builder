/**
 * Shared types for the basic-blog example.
 *
 * `BlogEntry` mirrors the shape returned by `getCollection("blog")` from
 * `zfb/content`. Keep `BlogFrontmatter` in step with the collection's
 * `schema` in `zfb.config.ts` — the schema is what `zfb check` validates
 * post frontmatter against, this type is what the routes see.
 */
import type { ContentProps } from "@takazudo/zfb/content";
import type { Description } from "@takazudo/zfb/zudo-react";

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
  /**
   * Stable bridge lookup key — `mdx://blog/<slug>` in v0. See the
   * `zfb/content` SDK docs for the full bridge contract.
   */
  module_specifier: string;
  /**
   * Renderable component for this entry. Pass `components` to override
   * specific HTML tags (or to inject custom JSX components used inside
   * MDX, e.g. `<Note>`):
   *
   * ```tsx
   * import { defaultComponents } from "@takazudo/zfb";
   *
   * <post.Content components={{ ...defaultComponents }} />
   * ```
   *
   * Overrides that should apply to every entry belong in the project-root
   * `mdx-components.tsx` instead of every call site.
   */
  Content: (props: ContentProps) => Description;
};
