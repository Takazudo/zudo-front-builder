import DefaultLayout from "../../layouts/default";

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
 * Per-post route. The `paths()` export is what the renderer evaluates to
 * expand the dynamic `[slug]` segment into one concrete route per entry in
 * the `blog` collection.
 */
export async function paths() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("blog")) as BlogEntry[];
  return posts.map((post) => ({
    params: { slug: post.slug },
    props: { post },
  }));
}

type Props = {
  post: BlogEntry;
};

export default function BlogPostPage({ post }: Props) {
  const tags = post.data.tags ?? [];
  return (
    <DefaultLayout title={post.data.title}>
      <article>
        <h1>{post.data.title}</h1>
        <p class="post-meta">
          <time dateTime={post.data.date}>{post.data.date}</time>
        </p>
        {/* Body is markdown rendered to HTML by the collection loader. */}
        <div
          class="post-body"
          dangerouslySetInnerHTML={{ __html: post.body }}
        />
        {tags.length > 0 ? (
          <ul class="tag-list">
            {tags.map((tag) => (
              <li key={tag}>
                <a href={`/tags/${tag}`}>#{tag}</a>
              </li>
            ))}
          </ul>
        ) : null}
        <p>
          <a href="/">← Back home</a>
        </p>
      </article>
    </DefaultLayout>
  );
}
