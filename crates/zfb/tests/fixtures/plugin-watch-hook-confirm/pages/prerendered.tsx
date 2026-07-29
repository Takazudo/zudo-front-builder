import { note } from "virtual:note";

// PRERENDERED (no `prerender = false`) — the sibling of `index.tsx`, and
// the half of this fixture that pins issue #2181. A prerendered route's
// HTML is committed to `.zfb-build/dev-pages/` and re-served from disk
// until something marks the page stale; `plugin-watched/note.txt`
// classifies as `PathClass::Unclassified` with no dependency-graph edge
// (the loader reads it through `node:fs`), so before #2181 the tick that
// refreshed `virtual:note` selected ZERO pages and this route served
// V1 forever. The orchestrator's plugin-watch block now marks
// `PageSelection::All` for any watch target, which is what re-renders
// this page. `index.tsx` is SSR-only and would pass either way — see the
// test file's header comment.

export default function PrerenderedNotePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>plugin-watch-hook prerendered confirm fixture</title>
      </head>
      <body>
        <h1>PLUGIN_WATCH_PRERENDERED_CONFIRM</h1>
        <p id="note">{note}</p>
      </body>
    </html>
  );
}
