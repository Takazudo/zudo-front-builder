// #4750: required catch-all; zero segments must stay a 404.
export const prerender = false;

export default function DocsPage({ params }: { params: { slug?: string } }) {
  return (
    <html lang="en">
      <body>{`SLASH_DOCS:slug=${params.slug ?? "<none>"}`}</body>
    </html>
  );
}
