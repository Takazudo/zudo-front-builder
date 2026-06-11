/**
 * Dynamic per-post page at `/posts/[slug]`, modeled on the node-free
 * template's `[slug].tsx`. Renders the post body via the
 * `entry.Content` contract so body markers from the markdown source
 * appear in the served HTML (#1018, scenarios 1-2).
 */
import { defaultComponents } from "@takazudo/zfb";
import type { ContentProps } from "@takazudo/zfb/content";
import { SharedNote } from "../../components/shared-note";

// Structural alias for the JSX-element shape returned by `entry.Content`
// (mirrors `zfb/content`'s `ContentElement` so this fixture stays
// renderer-agnostic).
type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

type Post = {
  slug: string;
  data: { title: string };
  Content: (props: ContentProps) => ContentElement;
};

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return posts.map((post) => ({
    params: { slug: post.slug },
    props: { post },
  }));
}

type Props = {
  post: Post;
};

export default function PostPage({ post }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>{post.data.title}</title>
      </head>
      <body>
        <h1>{post.data.title}</h1>
        <SharedNote />
        <article>
          <post.Content components={{ ...defaultComponents }} />
        </article>
      </body>
    </html>
  );
}
