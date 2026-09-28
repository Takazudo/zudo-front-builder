export default function DefaultLayout({ children }: { children: unknown }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Owned SSR fixture</title>
      </head>
      <body>{children}</body>
    </html>
  );
}
