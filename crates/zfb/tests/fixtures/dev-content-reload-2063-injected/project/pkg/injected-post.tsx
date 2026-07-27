/**
 * Dynamic INJECTED per-post page at `/injected-posts/[slug]`, sourced
 * from the out-of-root `posts` collection (`../shared-content/posts`).
 * This is issue #2094's matrix cell (a)+(c)-combo repro: the ONLY route
 * that ever consumes this collection is this plugin-injected dynamic
 * route — the project has no `pages/` directory at all, so
 * `routes_by_source` never carries an entry for it. Per epic #2092's
 * world-fact #2, a dynamic injected route is re-staled via
 * `restale_dynamic_injected` (`crates/zfb/src/commands/dev.rs`)
 * WITHOUT ever pushing to the tick's SSE-visible `tick_stale` channel —
 * so a content edit here is predicted to re-render server-side (fresh
 * bytes on the next request) while emitting ZERO `page` SSE events,
 * because the dev session's whole known-page universe is empty and
 * `outcome_to_events`'s gate never fires.
 */
import type { ContentProps } from "@takazudo/zfb/content";

type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

type Post = {
  slug: string;
  data: { title: string };
  Content: (props: ContentProps) => ContentElement;
};

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  return posts.map((post) => ({
    params: { slug: post.slug },
    props: { post },
  }));
}

type Props = {
  post: Post;
};

export default function InjectedPostPage({ post }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>{post.data.title}</title>
      </head>
      <body>
        <h1>{post.data.title}</h1>
        <article>
          <post.Content components={{}} />
        </article>
      </body>
    </html>
  );
}
