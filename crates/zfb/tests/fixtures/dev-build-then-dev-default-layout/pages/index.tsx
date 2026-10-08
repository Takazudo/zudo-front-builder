import { getCollection } from "@takazudo/zfb/content";
import defaultComponents from "../mdx-components";
import { components as pageComponents } from "./_mdx-components";

export default function HomePage() {
  const post = getCollection("blog").find((entry) => entry.slug === "entry");
  if (!post)
    return (
      <html>
        <body>Missing entry</body>
      </html>
    );
  return (
    <html lang="en">
      <head>
        <title>Default layout reproduction</title>
      </head>
      <body>
        <main>
          <post.Content components={{ ...defaultComponents, ...pageComponents }} />
        </main>
      </body>
    </html>
  );
}
