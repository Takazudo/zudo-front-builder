import { Island } from "@takazudo/zfb";

import { Counter } from "../components/counter";
import { Gallery } from "../components/gallery";

/**
 * Minimal built-site-smoke fixture page (issue #1401). Deliberately tiny —
 * no layout, no content collections, no tailwind — so `zfb build` stays
 * fast. The only thing this page needs to prove is that a real interactive
 * island survives the full build -> static-serve -> real-Chromium path.
 *
 * `when="load"` hydrates as soon as the client runtime script has loaded
 * (Astro's `client:load` equivalent) — the most deterministic scheduling
 * strategy for a CI smoke test, avoiding `requestIdleCallback`/
 * `IntersectionObserver` timing variance from `"idle"`/`"visible"`.
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>built-site-smoke fixture</title>
      </head>
      <body>
        <h1>built-site-smoke fixture</h1>
        <Island when="load">
          <Counter />
        </Island>
        <Island when="load">
          <Gallery />
        </Island>
      </body>
    </html>
  );
}
