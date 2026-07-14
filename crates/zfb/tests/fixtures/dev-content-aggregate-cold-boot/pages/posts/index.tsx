type Post = {
  slug: string;
  data: { title: string; date: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return {
    props: { posts: [...posts].sort((a, b) => b.data.date.localeCompare(a.data.date)) },
  };
}

export default function PostIndex({ posts }: { posts: Post[] }) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Post index</title>
      </head>
      <body>
        <h1>Post index</h1>
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>{post.data.title}</li>
          ))}
        </ul>
      </body>
    </html>
  );
}
