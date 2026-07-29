import fs from "node:fs";
import path from "node:path";

// Issue #2170 (epic #2166 "Plugin Watch Hook", confirm e2e): registers a
// virtual module whose loader reads `plugin-watched/note.txt` directly via
// `node:fs` — a read the dev bundler's static-import scan cannot see on its
// own, exactly the gap the `watchFiles` registration option (#2167) exists
// to close. `plugin-watched/` is deliberately NOT one of the default watch
// roots (`pages`, `content`, `components`, `layouts`, `styles`, `data`,
// `src`), so this loader's freshness depends entirely on the `watchFiles`
// entry below being both registered with the live watcher and re-invoked
// by the pre-tick refresh hook (#2168/#2169).
export default {
  name: "plugin-watch-hook-confirm-preset",
  setup({ projectRoot, addVirtualModule }) {
    const notePath = path.join(projectRoot, "plugin-watched", "note.txt");
    addVirtualModule(
      "virtual:note",
      () => `export const note = ${JSON.stringify(fs.readFileSync(notePath, "utf8").trim())};`,
      { watchFiles: [notePath] },
    );
  },
};
