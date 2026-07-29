import { note } from "virtual:note";

// SSR-only (not prerendered): a plugin watch-file edit under
// `plugin-watched/` (an unrecognized-segment, in-project path) does not
// mark a PRERENDERED page's on-disk output stale — only CSS/islands/
// client-scripts consumers and SSR-only routes actually see fresh content
// on the next request. See the test file's header comment for the traced
// classification/finding this fixture is built around.
export const prerender = false;

/**
 * Renders the plugin-registered `virtual:note` module's content. The
 * confirm e2e (issue #2170) edits `plugin-watched/note.txt` on disk and
 * asserts this page's served HTML picks up the new value without a dev
 * server restart — proving both the `watchFiles` registration (#2167) and
 * the pre-tick refresh invalidation (#2168/#2169) end to end.
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>plugin-watch-hook confirm fixture</title>
      </head>
      <body>
        <h1>PLUGIN_WATCH_CONFIRM</h1>
        <p id="note">{note}</p>
      </body>
    </html>
  );
}
