// Nested dynamic page at "/[lang]/[slug]". Two dynamic segments
// in one path; `paths()` returns the cartesian product.

export const meta = {
  title: "Localized",
  layout: "default",
};

const langs = ["en", "ja"] as const;
const slugs = ["welcome", "goodbye"] as const;

type Lang = (typeof langs)[number];
type Slug = (typeof slugs)[number];

const greetings: Record<Lang, Record<Slug, string>> = {
  en: { welcome: "Welcome", goodbye: "Goodbye" },
  ja: { welcome: "ようこそ", goodbye: "さようなら" },
};

export function paths() {
  const out: {
    params: { lang: Lang; slug: Slug };
    props: { lang: Lang; slug: Slug; greeting: string };
  }[] = [];
  for (const lang of langs) {
    for (const slug of slugs) {
      out.push({
        params: { lang, slug },
        props: { lang, slug, greeting: greetings[lang][slug] },
      });
    }
  }
  return out;
}

export default function Localized({
  lang,
  slug,
  greeting,
}: {
  lang: Lang;
  slug: Slug;
  greeting: string;
}) {
  return (
    <section className="page page-localized">
      <h2>
        {lang} / {slug}
      </h2>
      <p>{greeting}</p>
    </section>
  );
}
