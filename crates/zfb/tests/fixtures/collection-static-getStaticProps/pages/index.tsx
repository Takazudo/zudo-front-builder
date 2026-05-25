type Post = {
  slug: string;
  data: { title: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return { props: { posts, count: posts.length } };
}

type Props = {
  posts: Post[];
  count: number;
};

export default function HomePage({ posts, count }: Props) {
  return (
    <html lang="en">
      <head>
        <title>collection-static-getStaticProps fixture</title>
      </head>
      <body>
        <h1>Posts</h1>
        {/* Render the count so the test can assert on a concrete number */}
        <p data-testid="count">post-count:{count}</p>
        <ul>
          {posts.map((p) => (
            <li key={p.slug}>{p.data.title}</li>
          ))}
        </ul>
      </body>
    </html>
  );
}
