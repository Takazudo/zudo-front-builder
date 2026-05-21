import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

// CCResDoc probe (issue #349): render the user's ~/.claude/CLAUDE.md as the
// single page served from the Tauri sidecar.
//
// IMPORTANT: this is `getStaticProps` — i.e. SSG, executed inside the build
// orchestrator's render loop, NOT per request. The issue body asked for
// `prerender = false` (request-time SSR), but the zfb dev server does not
// execute SSR handlers today (see findings doc §3.2). The build-time read is
// the closest current zfb gets to "fresh CLAUDE.md".
//
// Hot reload caveat: zfb-watcher is rooted at project_root, so edits to
// $HOME/.claude/CLAUDE.md do NOT trigger a rebuild — the page only refreshes
// when something inside this project changes. See findings doc §3.3.
export async function getStaticProps() {
  const path = join(homedir(), ".claude", "CLAUDE.md");
  let body = "";
  let error: string | null = null;
  try {
    body = await readFile(path, "utf8");
  } catch (e) {
    error = e instanceof Error ? e.message : String(e);
  }
  return { props: { path, body, error } };
}

type Props = { path: string; body: string; error: string | null };

export default function ClaudeDocPage({ path, body, error }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>CCResDoc probe — CLAUDE.md</title>
      </head>
      <body>
        <h1>CLAUDE.md</h1>
        <p>
          <small>source: {path}</small>
        </p>
        {error ? <pre data-error>{error}</pre> : <pre>{body}</pre>}
      </body>
    </html>
  );
}
