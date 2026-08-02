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
      description="A small, complete zfb blog: content collections, markdown features, an island, and Tailwind."
    >
      <h1 class="text-3xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-50">
        basic-blog
      </h1>
      <p class="mt-4 text-lg leading-relaxed">
        A small but complete zfb site. It has a content collection, a set of markdown features
        turned on, one client island, and Tailwind for styling — enough to show the shape of a real
        project without hiding it behind abstractions.
      </p>
      <p class="mt-4">
        Start at <code class="text-sm">pages/index.tsx</code> and follow the imports outward. The{" "}
        <a href="/about" class="text-accent hover:underline">
          about page
        </a>{" "}
        maps every directory to what it does.
      </p>

      <h2
        id="posts"
        class="mt-14 scroll-mt-8 text-xs font-semibold tracking-[0.08em] text-neutral-500 uppercase"
      >
        Posts
      </h2>
      <ul class="mt-6 divide-y divide-neutral-200 dark:divide-neutral-800">
        {posts.map((post) => (
          <li key={post.slug} class="py-5 first:pt-0">
            <a href={`/blog/${post.slug}`} class="group block">
              <h3 class="font-medium text-neutral-900 group-hover:text-accent dark:text-neutral-100">
                {post.data.title}
              </h3>
              {post.data.description ? (
                <p class="mt-1 text-sm leading-relaxed text-neutral-600 dark:text-neutral-400">
                  {post.data.description}
                </p>
              ) : null}
              <time
                dateTime={post.data.date}
                class="mt-2 block text-xs text-neutral-500 tabular-nums"
              >
                {post.data.date}
              </time>
            </a>
          </li>
        ))}
      </ul>
    </DefaultLayout>
  );
}
