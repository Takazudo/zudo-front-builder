/**
 * Home page — lists every entry in the `posts` collection by its
 * frontmatter title, so the dev E2E harness can assert frontmatter
 * fan-out (#1018, scenario 3: editing a post's title must reach this
 * listing without a dev-server restart).
 */
import { SharedNote } from "../components/shared-note";

type Post = {
  slug: string;
  data: { title: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  const sorted = [...posts].sort((a, b) => a.slug.localeCompare(b.slug));
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
        <title>dev-loop-basic fixture</title>
      </head>
      <body>
        <h1>dev-loop-basic</h1>
        <SharedNote />
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>
              <a href={`/posts/${post.slug}`}>{post.data.title}</a>
            </li>
          ))}
        </ul>
      </body>
    </html>
  );
}
