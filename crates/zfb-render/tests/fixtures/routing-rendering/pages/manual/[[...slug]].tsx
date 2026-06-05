// Optional catchall page at "/manual/[[...slug]]" (#812). Matches the
// bare directory URL (`/manual`, slug = []) AND arbitrary-depth nested
// paths. `paths()` includes the explicit `slug: []` entry so the harness
// can assert the zero-segment page builds at `manual/index.html`.

export const meta = {
  title: "Manual",
  layout: "default",
};

const stubPages: { slug: string[]; body: string }[] = [
  { slug: [], body: "Manual home." },
  { slug: ["setup", "quick"], body: "Quick setup." },
];

export function paths() {
  return stubPages.map((p) => ({
    params: { slug: p.slug },
    props: { slug: p.slug, body: p.body },
  }));
}

export default function ManualPage({ slug, body }: { slug: string[]; body: string }) {
  return (
    <section className="page page-manual">
      <h2>{slug.length === 0 ? "Manual" : slug.join(" / ")}</h2>
      <p>{body}</p>
    </section>
  );
}
