/**
 * Per-post page — renders a single post from the `posts` collection.
 *
 * `paths` enumerates all post slugs so zfb can statically generate one
 * HTML file per post. `getStaticProps` fetches the specific post entry.
 */
export async function paths() {
  const { getCollection } = await import("zfb/content");
  const posts = await getCollection("posts");
  return posts.map((p: { slug: string }) => ({ params: { slug: p.slug } }));
}

export async function getStaticProps({ params }: { params: { slug: string } }) {
  const { getCollection } = await import("zfb/content");
  const posts = await getCollection("posts");
  const post = posts.find((p: { slug: string }) => p.slug === params.slug);
  if (!post) throw new Error(`Post not found: ${params.slug}`);
  return { props: { post } };
}

type Post = {
  slug: string;
  data: { title: string; date: string; description?: string };
  Content: () => JSX.Element;
};

type Props = { post: Post };

export default function PostPage({ post }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{post.data.title} · node-free</title>
      </head>
      <body>
        <a href="/">← Home</a>
        <h1>{post.data.title}</h1>
        <time dateTime={post.data.date}>{post.data.date}</time>
        <post.Content />
      </body>
    </html>
  );
}
