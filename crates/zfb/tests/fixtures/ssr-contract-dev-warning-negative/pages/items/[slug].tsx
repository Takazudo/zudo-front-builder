// Negative control (#2357, epic #2351): a dynamic SSR route whose first
// parameter is a DESTRUCTURED `{ params }` — the correct dynamic-route
// shape (`DefaultExportFirstParam::Destructured`). The detector's gate
// requires a plain binding identifier for its first parameter, so a
// destructuring pattern must never fire it.
export const prerender = false;

export default function ItemPage({ params }: { params: { slug: string } }) {
  return (
    <html lang="en">
      <head>
        <title>item</title>
      </head>
      <body>ITEM_OK:{params.slug}</body>
    </html>
  );
}
