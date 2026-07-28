/**
 * Dynamic per-post page at `/posts/[slug]`, sourced from the `posts`
 * collection whose configured root lives OUTSIDE this project
 * (`../shared-content/posts`, `allowOutsideRoot: true` in
 * zfb.config.json — mirrors `dev-out-of-root-basic`, #1552). This is
 * cell (c1) of issue #2094's variant matrix: an ORDINARY out-of-root
 * collection with NO `external_invalidation` narrowing hook (the #1038
 * baseline) — the healthy-characterization control paired with the
 * zero-pages injected-route cell in `dev-content-reload-2063-injected/`.
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
