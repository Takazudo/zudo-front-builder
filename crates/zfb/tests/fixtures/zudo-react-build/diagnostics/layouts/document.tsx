export default function Document({ children }: { children: unknown }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Diagnostics fixture</title>
      </head>
      <body>{children}</body>
    </html>
  );
}
