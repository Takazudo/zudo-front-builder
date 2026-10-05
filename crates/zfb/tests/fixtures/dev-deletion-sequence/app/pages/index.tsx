import { getCollection } from "@takazudo/zfb/content";

export default function Index() {
  const docs = getCollection("docs");
  return (
    <html>
      <body>
        {docs.map((entry) => (
          <p key={entry.slug}>
            {entry.slug}: {String(entry.data.title)}
          </p>
        ))}
      </body>
    </html>
  );
}
