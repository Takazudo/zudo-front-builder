import { marker } from "../node_modules/repro-realpath-dep/index.js";
import { LOCAL_ALIAS_VALUE } from "@fixture/alias";

export const prerender = false;

export default function RuntimePage() {
  return (
    <html lang="en">
      <head>
        <title>SSR realpath dependency</title>
      </head>
      <body>
        <p>
          {marker}:{LOCAL_ALIAS_VALUE}
        </p>
      </body>
    </html>
  );
}
