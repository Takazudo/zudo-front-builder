import DefaultLayout from "../../layouts/default";
import type { BlogEntry } from "../../lib/types";

/**
 * One static page per unique tag. The renderer evaluates `paths()` once,
 * collects every tag mentioned across the collection, and emits a route for
 * each.
 */
export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("blog")) as BlogEntry[];
  const byTag = new Map<string, BlogEntry[]>();
  for (const post of posts) {
    for (const tag of post.data.tags ?? []) {
      const bucket = byTag.get(tag) ?? [];
      bucket.push(post);
      byTag.set(tag, bucket);
    }
  }
  return Array.from(byTag.entries()).map(([tag, group]) => {
    // Don't mutate the bucket array in place — see also pages/index.tsx.
    const sorted = [...group].sort((a, b) => b.data.date.localeCompare(a.data.date));
    return {
      params: { tag },
      props: { tag, posts: sorted },
    };
  });
}

type Props = {
  tag: string;
  posts: BlogEntry[];
};

export default function TagPage({ tag, posts }: Props) {
  return (
    <DefaultLayout title={`#${tag}`}>
      <h1>
        Posts tagged <code>#{tag}</code>
      </h1>
      <ul class="post-list">
        {posts.map((post) => (
          <li key={post.slug}>
            <a href={`/blog/${post.slug}`}>{post.data.title}</a>
            <div class="post-meta">
              <time dateTime={post.data.date}>{post.data.date}</time>
            </div>
          </li>
        ))}
      </ul>
      <p>
        <a href="/">← Back home</a>
      </p>
    </DefaultLayout>
  );
}
