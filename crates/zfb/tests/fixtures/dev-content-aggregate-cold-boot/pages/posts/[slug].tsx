import type { ContentProps } from "@takazudo/zfb/content";

type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

type Post = {
  slug: string;
  data: { title: string; date: string; tags?: string[] };
  Content: (props: ContentProps) => ContentElement;
};

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return posts.map((post) => ({ params: { slug: post.slug }, props: { post } }));
}

export default function EntryPage({ post }: { post: Post }) {
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
