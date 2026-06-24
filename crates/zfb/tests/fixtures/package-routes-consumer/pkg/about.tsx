/**
 * Static package route page: /preset-about
 *
 * No CSS Modules, no import.meta.glob — source transforms are skipped for
 * package route pages (see Z1b sharp edge in epic #1191).
 */
import { siteTitle } from "./site-meta";

export default function AboutPage() {
  return (
    <html lang="en">
      <head>
        <title>About — {siteTitle}</title>
      </head>
      <body>
        <h1>CONSUMER_PRESET_ABOUT_MARKER</h1>
        <p>Site: {siteTitle}</p>
      </body>
    </html>
  );
}
