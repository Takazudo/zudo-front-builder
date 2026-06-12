/**
 * Non-HTML feed route (#1027, lazy E2E scenario c). Lists every post
 * title so a content edit affects this output. Feed-style routes have
 * no browser tab subscribed to SSE — under the lazy dev-render default
 * the request-time render is their ONLY refresh path, which is exactly
 * what the harness asserts.
 *
 * Filename convention: `feed.xml.tsx` → `/feed.xml` with
 * `application/xml` Content-Type (see docs concepts/non-html-pages).
 */
type Post = {
  slug: string;
  data: { title: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  const sorted = [...posts].sort((a, b) => a.slug.localeCompare(b.slug));
  return { props: { titles: sorted.map((post) => post.data.title) } };
}

type Props = {
  titles: string[];
};

export default function Feed({ titles }: Props): string {
  const items = titles.map((title) => `  <title>${title}</title>`).join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>\n<feed>\n${items}\n</feed>\n`;
}
