import { count } from "virtual:count";
import { listing, note } from "virtual:listing";

// SSR-only: rendered fresh on every request (store-refresh half).
export const prerender = false;

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>plugin-watch-directories fixture</title>
      </head>
      <body>
        <p id="listing">{listing}</p>
        <p id="count">{count}</p>
        <p id="note">{note}</p>
      </body>
    </html>
  );
}
