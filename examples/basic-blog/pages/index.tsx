import DefaultLayout from "../layouts/default";

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

/**
 * The homepage lists every post in the `blog` collection, newest first. We
 * intentionally do not paginate here — pagination is shown on
 * `/blog/page/[page]` so each route demonstrates exactly one concept.
 */
export async function getStaticProps() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("blog")) as BlogEntry[];
  posts.sort((a, b) => b.data.date.localeCompare(a.data.date));
  return { props: { posts } };
}

type Props = {
  posts: BlogEntry[];
};

export default function HomePage({ posts }: Props) {
  return (
    <DefaultLayout title="basic-blog · zfb example">
      <h1>basic-blog</h1>
      <p>
        A minimal real zfb site. Every file under this directory maps onto a
        single concept in the docs.
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
