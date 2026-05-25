/**
 * Dynamic per-post page at `/posts/[slug]`.
 *
 * Driven by the `posts` content collection (see `zfb.config.json`).
 * `getCollection("posts")` returns every `.md` file under
 * `content/posts/` with its frontmatter parsed; `paths()` expands the
 * `[slug]` segment into one concrete route per entry; `getStaticProps`
 * receives the per-route props and forwards them to the page component.
 */
import { defaultComponents } from "@takazudo/zfb";
import type { ContentProps } from "@takazudo/zfb/content";

// Structural alias for the JSX-element shape returned by `entry.Content`
// (mirrors `zfb/content`'s `ContentElement` so this template stays
// renderer-agnostic — both Preact's and React's `jsx-runtime` accept it).
type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

type Post = {
  slug: string;
  data: { title: string; date?: string };
  body: string;
  module_specifier: string;
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

export default function PostPage({ post }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{post.data.title} · node-free · zfb</title>
      </head>
      <body>
        <p>
          <a href="/">← Home</a>
        </p>
        <article>
          <h1>{post.data.title}</h1>
          {post.data.date ? (
            <p>
              <time dateTime={post.data.date}>{post.data.date}</time>
            </p>
          ) : null}
          {/*
            Body rendering — `entry.Content` contract.

            `getCollection("posts")` returns each entry with a `Content`
            component (see `zfb/content`). Rendering via
            `<post.Content components={{ ...defaultComponents }} />`
            applies the htmlOverrides convention (passthroughs for
            `<p>`, `<a>`, headings, lists, strong / em, etc.) so
            Markdown syntax in the post body produces real HTML rather
            than literal source text.

            Outside the production renderer (unit tests, dev sandboxes,
            and the v0 CLI), `Content` falls back to a
            `<pre data-zfb-content-fallback>` block printing the raw
            body with a `[zfb fallback render]` marker.
          */}
          <post.Content components={{ ...defaultComponents }} />
        </article>
      </body>
    </html>
  );
}
