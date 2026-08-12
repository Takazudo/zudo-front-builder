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
//
// Issue #2374 (epic #2368 "Plugin Diagnostics & Hygiene", confirm e2e)
// extends this preset with three more things the sibling e2e in this file
// asserts against captured `zfb dev` terminal output:
//
// - a setup-time `logger.info` call, proving the diagnostics epic's
//   log-rendering (#2369) reaches the terminal for the `setup` hook, not
//   just build hooks;
// - a `console.log` call inside the loader itself, proving the global
//   `console` redirection (#2373) attributes a virtual-module loader's own
//   console writes back to this plugin;
// - the `THROW-ON-REINVOKE` sentinel below, letting the e2e force this
//   SAME loader to throw on a watch-triggered re-invoke (never on the
//   original #2170 scenario's V1/V2/... content, which never writes this
//   exact value) so it can assert the failed-reinvoke diagnostic
//   (#2370/dev.rs's `fmt_plugin_refresh_failures`) surfaces exactly once
//   while the served content stays last-good, then recovers on the next
//   successful re-invoke.
export default {
  name: "plugin-watch-hook-confirm-preset",
  setup({ projectRoot, addVirtualModule, logger }) {
    logger.info("plugin-watch-hook-confirm-preset: setup ran");
    const notePath = path.join(projectRoot, "plugin-watched", "note.txt");
    addVirtualModule(
      "virtual:note",
      () => {
        const raw = fs.readFileSync(notePath, "utf8").trim();
        console.log(
          `plugin-watch-hook-confirm-preset: virtual:note loader read ${JSON.stringify(raw)}`,
        );
        if (raw === "THROW-ON-REINVOKE") {
          throw new Error("virtual:note loader intentionally failed (issue #2374 confirm e2e)");
        }
        return `export const note = ${JSON.stringify(raw)};`;
      },
      { watchFiles: [notePath] },
    );
  },
};
