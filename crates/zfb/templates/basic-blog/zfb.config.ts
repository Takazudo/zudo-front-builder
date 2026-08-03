import { defineConfig } from "zfb/config";

/**
 * Every option zfb already defaults to is omitted on purpose — `outDir`,
 * `publicDir`, and `tailwind` are all left implicit so this file only
 * shows the decisions this project actually made.
 *
 * `zfb/config` is a bare specifier the config loader aliases to an
 * internal stub at parse time, so the config is readable without any
 * package installed. `components/zfb-shim.d.ts` gives your editor and
 * `zfb check` the matching types.
 */
export default defineConfig({
  framework: "preact",

  collections: [
    {
      name: "blog",
      path: "content/blog",
      // Enforced by `pnpm typecheck` (`zfb check`), not at build time —
      // a post missing `title`/`date` is reported as a schema violation.
      schema: {
        type: "object",
        properties: {
          title: { type: "string" },
          date: { type: "string" },
          description: { type: "string" },
          tags: { type: "array", items: { type: "string" } },
        },
        required: ["title", "date"],
      },
    },
  ],

  markdown: {
    // A partial object only overrides the keys it names, so strikethrough
    // and tables keep their on-by-default values; task lists and footnotes
    // are the additional opt-ins.
    gfm: {
      taskListItem: true,
      footnoteDefinition: true,
    },
    features: {
      // `> [!NOTE]` blockquotes become <Note>/<Tip>/… components, resolved
      // through the project-root `mdx-components.tsx` map.
      githubAlerts: true,
      // Object form turns the feature on with all three sub-features
      // (diff markers, line highlight, word highlight) enabled.
      codeEnrichment: {},
      // Inserts a table of contents after a `## TOC` heading.
      headingMarkerToc: true,
    },
  },
});
