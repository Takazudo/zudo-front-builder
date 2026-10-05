import type { ContentProps } from "@takazudo/zfb/content";

type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};
type Doc = {
  slug: string;
  data: { title: string };
  Content: (props: ContentProps) => ContentElement;
};

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const docs = (await getCollection("docs")) as Doc[];
  return docs.map((doc) => ({ params: { slug: doc.slug }, props: { doc } }));
}

export default function Detail({ doc }: { doc: Doc }) {
  return (
    <html>
      <body>
        <h1>{doc.data.title}</h1>
        <article>
          <doc.Content components={{}} />
        </article>
      </body>
    </html>
  );
}
