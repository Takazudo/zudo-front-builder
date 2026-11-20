// #4750: a static sibling of `live/[slug]`; must win `/live/special/` too.
export const prerender = false;

export default function LiveSpecialPage() {
  return (
    <html lang="en">
      <body>{"SLASH_LIVE_SPECIAL"}</body>
    </html>
  );
}
