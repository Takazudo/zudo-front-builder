import DefaultLayout from "../../layouts/default";

type BlogEntry = {
  slug: string;
  frontmatter: {
    title: string;
    date: string;
    description?: string;
  };
  body: string;
};

type Props = {
  entry: BlogEntry;
};

export async function getStaticPaths() {
  // The zfb runtime resolves this against the `blog` collection defined in
  // `zfb.config.ts`. Each entry becomes a static route at /blog/<slug>.
  const { getCollection } = await import("zfb/content");
  const entries = await getCollection("blog");
  return entries.map((entry: BlogEntry) => ({
    params: { slug: entry.slug },
    props: { entry },
  }));
}

export default function BlogPostPage({ entry }: Props) {
  return (
    <DefaultLayout title={entry.frontmatter.title}>
      <article>
        <h1>{entry.frontmatter.title}</h1>
        <p>
          <time dateTime={entry.frontmatter.date}>{entry.frontmatter.date}</time>
        </p>
        <div dangerouslySetInnerHTML={{ __html: entry.body }} />
        <p>
          <a href="/">← Back home</a>
        </p>
      </article>
    </DefaultLayout>
  );
}
