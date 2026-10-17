import { getCollection } from "@takazudo/zfb/content";

// Uses the same collection/paths/Content contract as the basic-blog template.
export async function paths() {
  const posts = await getCollection("posts");
  return posts.map((post) => ({
    params: { slug: post.slug },
    props: { post },
  }));
}

export default function PostPage({ post }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>{post.data.title}</title>
      </head>
      <body>
        <article data-v8-fixture="post">
          <h1>{post.data.title}</h1>
          <post.Content />
        </article>
      </body>
    </html>
  );
}
