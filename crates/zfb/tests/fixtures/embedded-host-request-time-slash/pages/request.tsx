// #4750: request-time getStaticProps on a static SSR route.
export const prerender = false;

export async function getStaticProps() {
  const response = await fetch(`http://127.0.0.1:${__LOOPBACK_PORT__}/value`);
  return { props: { value: await response.text() } };
}

export default function RequestPage({ value }: { value: string }) {
  return (
    <html lang="en">
      <body>{`SLASH_REQUEST:value=${value}`}</body>
    </html>
  );
}
