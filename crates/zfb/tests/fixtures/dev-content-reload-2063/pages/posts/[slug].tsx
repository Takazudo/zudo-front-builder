/**
 * Dynamic per-post page at `/posts/[slug]`, sourced from the `posts` MDX
 * content collection — the #2063 repro harness's prerendered entry route.
 * Modeled on `dev-out-of-root-basic/project/pages/posts/[slug].tsx`
 * (#1552): same per-entry `paths()` + `entry.Content` render contract, an
 * in-root collection here instead of an out-of-root one.
 */
import type { ContentProps } from "@takazudo/zfb/content";

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
        <article>
          <post.Content components={{}} />
        </article>
      </body>
    </html>
  );
}
