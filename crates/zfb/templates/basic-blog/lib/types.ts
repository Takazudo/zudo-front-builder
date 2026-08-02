/**
 * Shared types for the basic-blog example.
 *
 * `BlogEntry` mirrors the shape returned by `getCollection("blog")` from
 * `zfb/content`. Keep `BlogFrontmatter` in step with the collection's
 * `schema` in `zfb.config.ts` — the schema is what `zfb check` validates
 * post frontmatter against, this type is what the routes see.
 */
import type { ContentProps } from "@takazudo/zfb/content";

export type BlogFrontmatter = {
  title: string;
  date: string;
  description?: string;
  tags?: string[];
};

/**
 * Structural JSX-element shape returned by `entry.Content`.
 *
 * Mirrors `zfb/content`'s `ContentElement` — kept locally as a structural
 * alias so the example doesn't need to import the renderer-facing JSX
 * type. Both Preact's and React's `jsx-runtime` accept this shape on
 * either side of the boundary.
 */
type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
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
  Content: (props: ContentProps) => ContentElement;
};
