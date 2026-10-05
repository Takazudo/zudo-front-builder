type Doc = { slug: string; data: { title: string } };

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const docs = (await getCollection("docs")) as Doc[];
  return { props: { docs } };
}

export default function Index({ docs }: { docs: Doc[] }) {
  return (
    <html>
      <body>
        {docs.map((doc) => (
          <p key={doc.slug}>
            {doc.slug}: {doc.data.title}
          </p>
        ))}
      </body>
    </html>
  );
}
