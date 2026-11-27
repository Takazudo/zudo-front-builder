import fs from "node:fs";
import path from "node:path";

// Issue #4894 confirm e2e: two loaders enumerate `plugin-dirs/` themselves
// and declare it with `watchDirectories`. `plugin-dirs/` is not a default
// watch root and no other dynamic-watch channel claims it, so its events
// reach the orchestrator only through the plugin directory registration.
//
// - `virtual:listing` lists every `**/*.md` file with its trimmed content,
//   throws when a file holds the `THROW-ON-REINVOKE` sentinel, and also
//   reads `plugin-watched/note.txt` through the unchanged `watchFiles`.
// - `virtual:count` counts the root's own `*.md` files and their bytes,
//   through two identical declarations (duplicates must dedupe).
//
// An edit of a top-level `.md` file affects BOTH loaders, which is what
// lets the e2e observe the all-or-nothing refresh: while `virtual:listing`
// throws, `virtual:count` must not publish its fresh value either.
export default {
  name: "plugin-watch-directories-preset",
  setup({ projectRoot, addVirtualModule }) {
    const root = path.join(projectRoot, "plugin-dirs");
    const notePath = path.join(projectRoot, "plugin-watched", "note.txt");

    const walk = (dir, prefix, out) => {
      for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
        if (entry.isDirectory()) walk(path.join(dir, entry.name), rel, out);
        else if (entry.name.endsWith(".md")) out.push(rel);
      }
      return out;
    };

    addVirtualModule(
      "virtual:listing",
      () => {
        const note = fs.readFileSync(notePath, "utf8").trim();
        if (!fs.existsSync(root)) {
          return `export const listing = "LIST:MISSING"; export const note = ${JSON.stringify(note)};`;
        }
        const entries = walk(root, "", [])
          .sort()
          .map((rel) => {
            const body = fs.readFileSync(path.join(root, rel), "utf8").trim();
            if (body === "THROW-ON-REINVOKE") {
              throw new Error(`virtual:listing loader intentionally failed on ${rel}`);
            }
            return `${rel}=${body}`;
          });
        const listing = `LIST:${entries.join("|")}:END`;
        return `export const listing = ${JSON.stringify(listing)}; export const note = ${JSON.stringify(note)};`;
      },
      {
        watchFiles: [notePath],
        watchDirectories: [{ path: root, include: ["**/*.md"] }],
      },
    );

    addVirtualModule(
      "virtual:count",
      () => {
        let files = 0;
        let bytes = 0;
        if (fs.existsSync(root)) {
          for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
            if (entry.isFile() && entry.name.endsWith(".md")) {
              files += 1;
              bytes += fs.readFileSync(path.join(root, entry.name)).length;
            }
          }
        }
        return `export const count = ${JSON.stringify(`COUNT:${files}:${bytes}`)};`;
      },
      {
        watchDirectories: [
          { path: root, include: ["*.md"] },
          { path: root, include: ["*.md"] },
        ],
      },
    );
  },
};
