// Paginated page at "/blog/page/[page]". Uses `paginate()` from
// the SDK module exposed as "zfb" (Sub 7's deliverable).

import { paginate } from "zfb";
import { posts, type Post } from "../../../content/posts";

export const meta = {
  title: "Blog (paginated)",
  layout: "blog",
};

export function paths() {
  return paginate({ items: posts, pageSize: 2 });
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
