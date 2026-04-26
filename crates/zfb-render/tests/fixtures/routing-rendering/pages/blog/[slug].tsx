// Dynamic page at "/blog/[slug]". `paths()` enumerates one
// route per post in the stub list.

import { posts, type Post } from "../../content/posts";

export const meta = {
  title: "Blog post",
  layout: "blog",
};

export function paths() {
  return posts.map((p) => ({
    params: { slug: p.slug },
    props: { post: p },
  }));
}

export default function BlogPost({ post }: { post: Post }) {
  return (
    <section className="page page-blog-post">
      <h2>{post.title}</h2>
      <p>{post.body}</p>
      <p className="slug">slug: {post.slug}</p>
    </section>
  );
}
