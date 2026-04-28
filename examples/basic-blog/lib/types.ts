/**
 * Shared types for the basic-blog example.
 *
 * `BlogEntry` mirrors the shape returned by `getCollection("blog")` from
 * `zfb/content` — the `data` payload follows the frontmatter schema we
 * keep authoritative in `zfb.config.json` (see also
 * `zfb.config.future.ts` for the typed sibling that will replace the
 * JSON form once the TS config loader lands).
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
   * import { defaultComponents } from "zfb";
   * import Note from "../components/note";
   *
   * <post.Content components={{ ...defaultComponents, Note }} />
   * ```
   */
  Content: (props: ContentProps) => ContentElement;
};
