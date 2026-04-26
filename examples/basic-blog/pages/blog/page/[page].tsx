import DefaultLayout from "../../../layouts/default";

type BlogEntry = {
  slug: string;
  data: {
    title: string;
    date: string;
    description?: string;
    tags?: string[];
  };
  body: string;
};

type PageData = {
  data: BlogEntry[];
  page: number;
  lastPage: number;
};

/**
 * Paginated index of blog posts. With `pageSize: 3` and 5 posts the runtime
 * emits `/blog/page/1` and `/blog/page/2`. The `paginate()` helper is the
 * canonical zfb shape for this; see /api/paginate in the docs.
 */
export async function paths() {
  const { getCollection } = await import("zfb/content");
  const { paginate } = await import("zfb/paginate");
  const posts = (await getCollection("blog")) as BlogEntry[];
  posts.sort((a, b) => b.data.date.localeCompare(a.data.date));
  return paginate(posts, { pageSize: 3, param: "page" });
}

type Props = {
  page: PageData;
};

export default function BlogIndexPage({ page }: Props) {
  return (
    <DefaultLayout title={`Posts — page ${page.page}`}>
      <h1>All posts</h1>
      <p class="post-meta">
        Page {page.page} of {page.lastPage}
      </p>
      <ul class="post-list">
        {page.data.map((post) => (
          <li key={post.slug}>
            <a href={`/blog/${post.slug}`}>{post.data.title}</a>
            <div class="post-meta">
              <time dateTime={post.data.date}>{post.data.date}</time>
              {post.data.description ? <> · {post.data.description}</> : null}
            </div>
          </li>
        ))}
      </ul>
      <nav class="pagination" aria-label="pagination">
        {page.page > 1 ? (
          <a href={`/blog/page/${page.page - 1}`}>← Newer</a>
        ) : (
          <span />
        )}
        {page.page < page.lastPage ? (
          <a href={`/blog/page/${page.page + 1}`}>Older →</a>
        ) : (
          <span />
        )}
      </nav>
    </DefaultLayout>
  );
}
