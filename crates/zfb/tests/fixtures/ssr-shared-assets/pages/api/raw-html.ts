export const prerender = false;

export default function RawHtml() {
  return new Response(
    '<html><head><link rel="stylesheet" href="/assets/styles.css"></head><body>raw</body></html>',
    { headers: { "Content-Type": "text/html; charset=utf-8" } },
  );
}
