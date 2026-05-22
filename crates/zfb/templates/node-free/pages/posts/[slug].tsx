/**
 * Dynamic per-post page at `/posts/[slug]`.
 *
 * Driven by the `posts` content collection (see `zfb.config.json`).
 * `getCollection("posts")` returns every `.md` file under
 * `content/posts/` with its frontmatter parsed; `paths()` expands the
 * `[slug]` segment into one concrete route per entry; `getStaticProps`
 * receives the per-route props and forwards them to the page component.
 */
type Post = {
  slug: string;
  data: { title: string; date?: string };
  body: string;
};

export async function paths() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return posts.map((post) => ({
    params: { slug: post.slug },
    props: { post },
  }));
}

type Props = {
  post: Post;
};

export default function PostPage({ post }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{post.data.title} · node-free · zfb</title>
      </head>
      <body>
        <p>
          <a href="/">← Home</a>
        </p>
        <article>
          <h1>{post.data.title}</h1>
          {post.data.date ? (
            <p>
              <time dateTime={post.data.date}>{post.data.date}</time>
            </p>
          ) : null}
          <p>{post.body}</p>
        </article>
      </body>
    </html>
  );
}
