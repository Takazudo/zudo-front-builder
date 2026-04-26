import DefaultLayout from "../../layouts/default";
import type { BlogEntry } from "../../lib/types";

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
        {/*
          Body rendering — v0 contract.

          The v0 `getCollection` stub in `zfb/content` returns `body` as the
          raw markdown source (frontmatter stripped), NOT pre-rendered
          HTML. The Rust-side markdown pipeline lives in `crates/zfb-content`
          and is not yet wired through to user code (blocked on the JS
          runtime decision in ADR-001). Until that lands, render the body
          as plain text inside a `<pre>` so:
            (a) we never emit `dangerouslySetInnerHTML` against an
                un-sanitised HTML string, and
            (b) the example still demonstrates the round-trip from
                markdown file → frontmatter → page output.

          TODO(zfb-content): switch back to a sanitised HTML render once
          the content engine ships end-to-end (rehype-sanitize at the
          pipeline level, or an explicit ZFB-INTERNAL safety contract).
        */}
        <pre class="post-body">{post.body}</pre>
        {tags.length > 0 ? (
          <ul class="tag-list">
            {tags.map((tag) => (
              <li key={tag}>
                <a href={`/tags/${encodeURIComponent(tag)}`}>#{tag}</a>
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
