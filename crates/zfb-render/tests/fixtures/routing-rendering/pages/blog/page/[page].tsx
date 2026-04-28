// Paginated page at "/blog/page/[page]". Uses an inline paginate helper
// (the "zfb" SDK paginate helper was planned for Sub 7 but has a
// different calling convention; inline avoids the "zfb" package import
// from the bundle's perspective and keeps the fixture self-contained).

import { posts, type Post } from "../../../content/posts";

export const meta = {
  title: "Blog (paginated)",
  layout: "blog",
};

type PaginateEntry<T> = {
  params: { page: string };
  props: { current: number; total: number; items: T[] };
};

function paginateLocal<T>(items: T[], pageSize: number): PaginateEntry<T>[] {
  const lastPage = Math.max(1, Math.ceil(items.length / pageSize));
  const out: PaginateEntry<T>[] = [];
  for (let page = 1; page <= lastPage; page++) {
    const start = (page - 1) * pageSize;
    out.push({
      params: { page: String(page) },
      props: {
        current: page,
        total: lastPage,
        items: items.slice(start, start + pageSize),
      },
    });
  }
  return out;
}

export function paths() {
  return paginateLocal(posts, 2);
}

type PageProps = {
  current: number;
  total: number;
  items: Post[];
};

export default function BlogPaginated({ current, total, items }: PageProps) {
  return (
    <section className="page page-blog-paginated">
      <h2>
        Page {current} of {total}
      </h2>
      <ul>
        {items.map((p) => (
          <li>
            <a href={`/blog/${p.slug}`}>{p.title}</a>
          </li>
        ))}
      </ul>
    </section>
  );
}
