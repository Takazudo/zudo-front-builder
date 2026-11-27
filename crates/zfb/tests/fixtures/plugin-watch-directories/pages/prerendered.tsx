import { count } from "virtual:count";
import { listing, note } from "virtual:listing";

// Prerendered: served from committed HTML until a tick invalidates it.

export default function PrerenderedPage() {
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
