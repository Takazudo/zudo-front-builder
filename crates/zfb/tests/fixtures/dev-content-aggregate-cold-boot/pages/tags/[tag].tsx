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
  const byTag = new Map<string, Post[]>();
  for (const post of posts) {
    for (const tag of post.data.tags ?? []) {
      const group = byTag.get(tag) ?? [];
      group.push(post);
      byTag.set(tag, group);
    }
  }
  return [...byTag].map(([tag, group]) => ({
    params: { tag },
    props: { tag, posts: [...group].sort((a, b) => b.data.date.localeCompare(a.data.date)) },
  }));
}

export default function TagPage({ tag, posts }: { tag: string; posts: Post[] }) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>{tag}</title>
      </head>
      <body>
        <h1>{tag}</h1>
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
