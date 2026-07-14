import type { ContentProps } from "@takazudo/zfb/content";

type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

type Post = {
  slug: string;
  data: { title: string; date: string };
  Content: (props: ContentProps) => ContentElement;
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return {
    props: { posts: [...posts].sort((a, b) => b.data.date.localeCompare(a.data.date)) },
  };
}

export default function PostIndex({ posts }: { posts: Post[] }) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Post index</title>
      </head>
      <body>
        <h1>Post index</h1>
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>
              <h2>{post.data.title}</h2>
              <post.Content components={{}} />
            </li>
          ))}
        </ul>
      </body>
    </html>
  );
}
