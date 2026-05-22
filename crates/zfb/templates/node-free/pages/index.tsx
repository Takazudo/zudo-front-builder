/**
 * Home page — lists all posts from the `posts` content collection.
 *
 * `getStaticProps` is evaluated at build time by zfb. The component
 * receives the resolved data as plain props, so no Node / pnpm is needed
 * at runtime.
 */
export async function getStaticProps() {
  const { getCollection } = await import("zfb/content");
  const posts = await getCollection("posts");
  return { props: { posts } };
}

type Post = {
  slug: string;
  data: { title: string; date: string; description?: string };
};

type Props = { posts: Post[] };

export default function HomePage({ posts }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>node-free · zfb</title>
      </head>
      <body>
        <h1>node-free</h1>
        <p>A minimal zfb site — no Node, no pnpm required.</p>
        <h2>Posts</h2>
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>
              <a href={`/posts/${post.slug}`}>{post.data.title}</a>
              {post.data.description ? <> — {post.data.description}</> : null}
            </li>
          ))}
        </ul>
      </body>
    </html>
  );
}
