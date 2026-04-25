// Catchall page at "/docs/[...slug]". Matches arbitrary depth
// path segments. `paths()` returns an array<segments> per doc
// so the harness can assert correct multi-segment URL building.

export const meta = {
  title: "Docs",
  layout: "default",
};

const stubDocs: { slug: string[]; body: string }[] = [
  { slug: ["intro"], body: "Intro doc." },
  { slug: ["guides", "install"], body: "Install guide." },
  { slug: ["guides", "config", "framework"], body: "Framework config." },
];

export function paths() {
  return stubDocs.map((d) => ({
    params: { slug: d.slug },
    props: { slug: d.slug, body: d.body },
  }));
}

export default function Doc({ slug, body }: { slug: string[]; body: string }) {
  return (
    <section className="page page-docs">
      <h2>{slug.join(" / ")}</h2>
      <p>{body}</p>
    </section>
  );
}
