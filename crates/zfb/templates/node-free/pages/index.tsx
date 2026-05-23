/**
 * Home page — lists every entry in the `posts` content collection.
 *
 * `getCollection("posts")` reads `.md` files under `content/posts/` at
 * build time. Under the embedded V8 host (the node-free path), the
 * snapshot is baked into the bundle, so `getCollection` resolves
 * synchronously from memory rather than reading `node:fs`.
 */
type Post = {
  slug: string;
  data: { title: string; date?: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  // Sort newest first when dates are present; preserve discovery order otherwise.
  const sorted = [...posts].sort((a, b) => (b.data.date ?? "").localeCompare(a.data.date ?? ""));
  return { props: { posts: sorted } };
}

type Props = {
  posts: Post[];
};

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
        <p>
          A minimal <code>zfb</code> site — no Node, no pnpm required. The <code>zfb</code> binary
          alone scaffolds, builds, and serves this page.
        </p>
        <h2>Posts</h2>
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>
              <a href={`/posts/${post.slug}`}>{post.data.title}</a>
              {post.data.date ? (
                <>
                  {" — "}
                  <time dateTime={post.data.date}>{post.data.date}</time>
                </>
              ) : null}
            </li>
          ))}
        </ul>
        <h2>Next steps</h2>
        <ul>
          <li>
            Edit <code>pages/index.tsx</code> to change this page.
          </li>
          <li>
            Add more posts by dropping <code>.md</code> files into <code>content/posts/</code>.
          </li>
          <li>
            Try the <a href="/about">About page</a> — a <code>.md</code> page entry that renders
            Markdown directly from <code>pages/about.md</code>.
          </li>
          <li>
            See the docs at{" "}
            <a href="https://github.com/Takazudo/zudo-front-builder">Takazudo/zudo-front-builder</a>
            .
          </li>
        </ul>
      </body>
    </html>
  );
}
