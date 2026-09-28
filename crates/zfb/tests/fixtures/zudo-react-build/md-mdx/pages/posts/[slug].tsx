import type { ContentProps } from "@takazudo/zfb/content";

type Post = {
  slug: string;
  data: { title: string };
  Content: (props: ContentProps) => unknown;
};

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return posts.map((post) => ({
    params: { slug: post.slug },
    props: { post },
  }));
}

export default function PostPage({ post }: { post: Post }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>{post.data.title}</title>
      </head>
      <body>
        <article>
          <post.Content components={{}} />
        </article>
      </body>
    </html>
  );
}
