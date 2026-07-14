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

type Page = {
  data: Post[];
};

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const { paginate } = await import("@takazudo/zfb/paginate");
  const posts = (await getCollection("posts")) as Post[];
  return paginate(
    [...posts].sort((a, b) => b.data.date.localeCompare(a.data.date)),
    {
      pageSize: 1,
      param: "page",
    },
  );
}

export default function PaginatedPosts({ page }: { page: Page }) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Paginated posts</title>
      </head>
      <body>
        <h1>Paginated posts</h1>
        <ul>
          {page.data.map((post) => (
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
