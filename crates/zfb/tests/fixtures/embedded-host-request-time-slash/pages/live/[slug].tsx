// #4750: request-time dynamic SSR route. `__LOOPBACK_PORT__` is replaced by
// the test with a loopback server whose body the test changes between
// requests, so a fresh value proves the handler ran for this request.
export const prerender = false;

export default async function LivePage({ params }: { params: { slug: string } }) {
  const response = await fetch(`http://127.0.0.1:${__LOOPBACK_PORT__}/value`);
  const value = await response.text();
  return (
    <html lang="en">
      <body>{`SLASH_LIVE:slug=${params.slug}|value=${value}`}</body>
    </html>
  );
}
