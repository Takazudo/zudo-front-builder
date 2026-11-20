// #4750: a prerendered sibling of the request-time `live/[slug]` route. With
// `trailingSlash: true` its canonical URL is `/live/about/`, which must stay
// this page rather than being admitted to `live/[slug]`.
export default function LiveAboutPage() {
  return (
    <html lang="en">
      <body>{"SLASH_LIVE_ABOUT_PRERENDERED"}</body>
    </html>
  );
}
