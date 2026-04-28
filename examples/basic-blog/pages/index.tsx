import DefaultLayout from "../layouts/default";
import type { BlogEntry } from "../lib/types";

/**
 * The homepage lists every post in the `blog` collection, newest first. We
 * intentionally do not paginate here — pagination is shown on
 * `/blog/page/[page]` so each route demonstrates exactly one concept.
 */
export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("blog")) as BlogEntry[];
  // Avoid mutating the array returned by `getCollection`: future
  // implementations may share the array between routes, and a sort()
  // call here would silently re-order it for everyone.
  const sorted = [...posts].sort((a, b) => b.data.date.localeCompare(a.data.date));
  return { props: { posts: sorted } };
}

type Props = {
  posts: BlogEntry[];
};

export default function HomePage({ posts }: Props) {
  return (
    <DefaultLayout title="basic-blog · zfb example">
      <h1>basic-blog</h1>
      <p>
        A minimal real zfb site. Every file under this directory maps onto a single concept in the
        docs.
      </p>
      <h2>Recent posts</h2>
      <ul class="post-list">
        {posts.map((post) => (
          <li key={post.slug}>
            <a href={`/blog/${post.slug}`}>{post.data.title}</a>
            <div class="post-meta">
              <time dateTime={post.data.date}>{post.data.date}</time>
              {post.data.description ? <> · {post.data.description}</> : null}
            </div>
          </li>
        ))}
      </ul>
    </DefaultLayout>
  );
}
