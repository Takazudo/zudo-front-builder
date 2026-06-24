/**
 * Consumer project's own home page. Must be unaffected by the preset's
 * injected routes — the overlay mechanism must not relocate or alter user
 * pages/ entries.
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <title>Consumer Home</title>
      </head>
      <body>
        <h1>CONSUMER_USER_HOME_MARKER</h1>
      </body>
    </html>
  );
}
