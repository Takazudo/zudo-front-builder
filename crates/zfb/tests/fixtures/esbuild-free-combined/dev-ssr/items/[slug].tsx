// Request-time SSR route (`prerender = false`). The dev test copies it into
// `pages/`; `zfb build` would need an adapter for it, so it stays out of
// the build fixture.
export const prerender = false;

export default function ItemPage({ params }: { params: { slug: string } }) {
  return (
    <html lang="en">
      <head>
        <title>request-time SSR</title>
      </head>
      <body>
        <p id="ssr">ZFB_REQUEST_SSR:{params.slug}</p>
      </body>
    </html>
  );
}
