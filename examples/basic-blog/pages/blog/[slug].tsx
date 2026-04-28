import { defaultComponents } from "@takazudo/zfb";

import Note from "../../components/note";
import DefaultLayout from "../../layouts/default";
import type { BlogEntry } from "../../lib/types";

/**
 * Per-post route. The `paths()` export is what the renderer evaluates to
 * expand the dynamic `[slug]` segment into one concrete route per entry in
 * the `blog` collection.
 */
export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
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
          Body rendering — `entry.Content` contract.

          `getCollection("blog")` returns each entry with a `Content`
          component (see `zfb/content`). We render the body by calling
          `<post.Content components={...} />` and pass:

          - `defaultComponents` from `zfb` — the htmlOverrides convention
            (passthroughs for `<p>`, `<a>`, headings, lists, etc.) ported
            from zudo-doc. Spread first so individual overrides on the
            right win on key collisions.
          - `Note` — a custom JSX component used inside the MDX post
            (`content/blog/hello-zfb.mdx`). MDX resolves `<Note>` against
            this map at evaluation time, which is what makes
            `<Note title="…">…</Note>` in the post body actually render.

          Outside the production renderer (unit tests, dev sandboxes,
          and the v0 CLI before ADR-001), `Content` falls back to a
          `<pre data-zfb-content-fallback>` block printing the raw body
          with a `[zfb fallback render]` marker. That keeps the example
          self-explanatory even when the bridge is absent.
        */}
        <post.Content components={{ ...defaultComponents, Note }} />
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
