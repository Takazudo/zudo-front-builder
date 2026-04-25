// Static page at "/blog". Lists all blog posts.

import { posts } from "../../content/posts";

export const meta = {
  title: "Blog",
  layout: "blog",
};

export default function BlogIndex() {
  return (
    <section className="page page-blog-index">
      <h2>All posts</h2>
      <ul>
        {posts.map((p) => (
          <li>
            <a href={`/blog/${p.slug}`}>{p.title}</a>
          </li>
        ))}
      </ul>
    </section>
  );
}
