import { getCollection } from "@takazudo/zfb/content";

export function paths() {
  return getCollection("docs").map((entry) => ({
    params: { slug: entry.slug },
    props: { slug: entry.slug },
  }));
}

export default function Page({ slug }: { slug: string }) {
  const entry = getCollection("docs").find((candidate) => candidate.slug === slug);
  return (
    <html>
      <body>{entry ? String(entry.data.title) : "Not found"}</body>
    </html>
  );
}
