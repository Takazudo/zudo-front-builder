import designRules from "../components/design-rules";

import DefaultLayout from "~/layouts/default";
import type { BlogEntry } from "~/lib/types";

/**
 * The homepage lists every post in the `blog` collection, newest first.
 * `getStaticProps` runs at build time; its return value is passed straight
 * to the page component as props.
 */
export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("blog")) as BlogEntry[];
  // Avoid mutating the array returned by `getCollection`: future
  // implementations may share the array between routes, and a sort()
  // call here would silently re-order it for everyone.
  const sorted = [...posts].sort((a, b) => b.data.date.localeCompare(a.data.date));
  return { props: { posts: sorted } };
}

type Props = {
  posts: BlogEntry[];
};

export default function HomePage({ posts }: Props) {
  return (
    <DefaultLayout
      title="basic-blog · a zfb starter"
      description="A small, complete zfb blog: content collections, markdown features, an island, and zudo-wind."
    >
      <h1 class="text-heading font-display font-strong tracking-tight text-ink">basic-blog</h1>
      <p class="mt-vsp-stack text-body max-w-reading">
        A small but complete zfb site. It has a content collection, a set of markdown features
        turned on, one client island, and zudo-wind for styling — enough to show the shape of a real
        project without hiding it behind abstractions.
      </p>
      <p class="mt-vsp-stack max-w-reading">
        Start at <code class="text-sm">pages/index.tsx</code> and follow the imports outward. The{" "}
        <a href="/about" class="text-accent hover:underline">
          about page
        </a>{" "}
        maps every directory to what it does.
      </p>

      <p class="mt-vsp-stack">
        <a
          href="#posts"
          class="inline-block bg-accent text-on-accent px-hsp-card py-vsp-stack rounded-panel font-strong"
        >
          Read the posts
        </a>
      </p>

      <h2
        id="posts"
        class="mt-vsp-section scroll-mt-8 text-xs font-semibold tracking-[0.08em] text-muted uppercase"
      >
        Posts
      </h2>
      <ul class="mt-vsp-stack grid gap-x-hsp-gutter gap-y-vsp-stack">
        {posts.map((post) => (
          <li
            key={post.slug}
            class="bg-surface border border-border rounded-panel px-hsp-card py-vsp-stack"
          >
            <a href={`/blog/${post.slug}`} class="group block">
              <h3 class="font-display text-section font-strong text-ink group-hover:text-accent">
                {post.data.title}
              </h3>
              {post.data.description ? (
                <p class="mt-vsp-stack text-body max-w-reading text-muted">
                  {post.data.description}
                </p>
              ) : null}
              <time
                datetime={post.data.date}
                class="mt-vsp-stack block text-caption text-muted tabular-nums"
              >
                {post.data.date}
              </time>
            </a>
          </li>
        ))}
      </ul>
      <section class="mt-vsp-section max-w-reading">
        <h2 class="font-display text-section font-strong">How this site is styled</h2>
        <ul class="mt-vsp-stack grid gap-y-vsp-stack">
          {designRules.map((rule) => (
            <li key={rule}>{rule}</li>
          ))}
        </ul>
        <p class="mt-vsp-stack">
          Start with <code>styles/design-system.css</code> and <code>DESIGN-NOTES.md</code>. The{" "}
          <a href="/blog/styling-with-zudo-wind" class="text-accent hover:underline">
            styling article
          </a>{" "}
          explains how owned values become utility classes and Markdown styles.
        </p>
      </section>
    </DefaultLayout>
  );
}
