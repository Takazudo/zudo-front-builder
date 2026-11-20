// #4750: optional catch-all; zero and nested segments.
export const prerender = false;

export default function OptPage({ params }: { params?: { slug?: string } }) {
  return (
    <html lang="en">
      <body>{`SLASH_OPT:slug=${params?.slug ?? "NONE"}`}</body>
    </html>
  );
}
