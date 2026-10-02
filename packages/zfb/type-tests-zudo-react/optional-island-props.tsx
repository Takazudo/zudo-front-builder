import { Island } from "@takazudo/zfb";

type Article = {
  id: string;
  description?: string | null;
  details: Array<{ label: string; description?: string | null }>;
};

type ArticleIslandProps = {
  locale?: string | null;
  currentSlug?: string | null;
  summary: { title: string; description?: string | null };
  articles: Article[];
};

function ArticleIsland(_props: ArticleIslandProps) {
  return <section />;
}

const packedOptionalPropsConsumer = (
  <Island>
    <ArticleIsland
      summary={{ title: "Summary with no description" }}
      articles={[
        {
          id: "omitted",
          details: [{ label: "nested description is omitted" }],
        },
        {
          id: "explicit-null",
          description: null,
          details: [{ label: "explicit null", description: null }],
        },
      ]}
    />
  </Island>
);

void packedOptionalPropsConsumer;
