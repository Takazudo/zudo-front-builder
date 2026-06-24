/**
 * Dynamic package route page: /preset-docs/[slug]
 *
 * Literal paths() (static extraction, no V8 round-trip). Each enumerated
 * page carries a per-slug marker via props so the test can assert params
 * reached the render.
 *
 * No CSS Modules, no import.meta.glob — source transforms are skipped for
 * package route pages (see Z1b sharp edge in epic #1191).
 */
import { siteTitle } from "./site-meta";

export function paths() {
  return [
    {
      params: { slug: "getting-started" },
      props: { title: "Getting Started", slugLabel: "getting-started" },
    },
    {
      params: { slug: "api-reference" },
      props: { title: "API Reference", slugLabel: "api-reference" },
    },
  ];
}

export default function DocsPage({ title, slugLabel }: { title: string; slugLabel: string }) {
  return (
    <html lang="en">
      <head>
        <title>
          {title} — {siteTitle}
        </title>
      </head>
      <body>
        <h1>CONSUMER_PRESET_DOCS_MARKER_{slugLabel}</h1>
        <p>{title}</p>
      </body>
    </html>
  );
}
