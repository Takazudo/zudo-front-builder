import { defaultComponents } from "@takazudo/zfb";

import DefaultLayout from "~/layouts/default";
import type { BlogEntry } from "~/lib/types";

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
    <DefaultLayout title={post.data.title} description={post.data.description}>
      <article>
        <header class="border-b border-neutral-200 pb-6 dark:border-neutral-800">
          <h1 class="text-2xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-50">
            {post.data.title}
          </h1>
          <div class="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2 text-xs text-neutral-500">
            <time dateTime={post.data.date} class="tabular-nums">
              {post.data.date}
            </time>
            {tags.map((tag) => (
              <span key={tag} class="rounded-full bg-neutral-100 px-2 py-0.5 dark:bg-neutral-900">
                #{tag}
              </span>
            ))}
          </div>
        </header>
        {/*
          Body rendering — the `entry.Content` contract.

          `getCollection("blog")` returns each entry with a `Content`
          component. Rendering it as `<post.Content components={...} />`
          passes element overrides down into the compiled markdown:

          - `defaultComponents` from `zfb` is the htmlOverrides convention
            (passthroughs for `<p>`, `<a>`, headings, lists, …).
          - The five alert components (`Note`, `Tip`, …) are NOT passed here.
            They come from the project-root `mdx-components.tsx`, which zfb
            merges in for every entry — so any post can use `> [!NOTE]`
            without each route wiring it up.

          Outside the production renderer (unit tests, dev sandboxes),
          `Content` falls back to a `<pre data-zfb-content-fallback>` block
          printing the raw body with a `[zfb fallback render]` marker.
        */}
        <div class="prose mt-8">
          <post.Content components={{ ...defaultComponents }} />
        </div>
      </article>
      <p class="mt-14 text-sm">
        <a href="/#posts" class="text-accent hover:underline">
          ← All posts
        </a>
      </p>
    </DefaultLayout>
  );
}
