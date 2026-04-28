import type { PaginatedPage } from "zfb/paginate";

import DefaultLayout from "../../../layouts/default";
import type { BlogEntry } from "../../../lib/types";

/**
 * Paginated index of blog posts. With `pageSize: 3` and 5 posts the runtime
 * emits `/blog/page/1` and `/blog/page/2`. The `paginate()` helper from
 * `zfb/paginate` is the canonical zfb shape for this — see /api/paginate
 * in the docs.
 */
export async function paths() {
  const { getCollection } = await import("zfb/content");
  const { paginate } = await import("zfb/paginate");
  const posts = (await getCollection("blog")) as BlogEntry[];
  const sorted = [...posts].sort((a, b) => b.data.date.localeCompare(a.data.date));
  return paginate(sorted, { pageSize: 3, param: "page" });
}

type Props = {
  page: PaginatedPage<BlogEntry>;
};

export default function BlogIndexPage({ page }: Props) {
  const hasPrev = page.page > 1;
  const hasNext = page.page < page.lastPage;
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
      {/*
        Pagination nav — only render the arrows that lead somewhere. Earlier
        revisions emitted empty `<span />` placeholders to keep the spacing
        symmetric, but those add noise to assistive tech. Direction glyphs
        are decorative and marked `aria-hidden`; the link text carries the
        actual semantic meaning.
      */}
      <nav class="pagination" aria-label="pagination">
        {hasPrev ? (
          <a class="pagination-prev" href={`/blog/page/${page.page - 1}`}>
            <span aria-hidden="true">←</span> Newer
          </a>
        ) : null}
        {hasNext ? (
          <a class="pagination-next" href={`/blog/page/${page.page + 1}`}>
            Older <span aria-hidden="true">→</span>
          </a>
        ) : null}
      </nav>
    </DefaultLayout>
  );
}
