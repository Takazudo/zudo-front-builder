#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  ARCHITECTURE_GROUPS,
  WIND_UTILITY_GROUPS,
  WIND_ENTRY_SLUGS,
  WIND_LEARN_SLUGS,
} from "../src/config/navigation-groups.mjs";

// zfb 2.20.2 drops MDX named imports; materialize only native component props.
// The surrounding authored content is retained, and membership stays in one source.
export const NAVIGATION_INDEX_MEMBERS = new Map([
  ["architecture/index", ARCHITECTURE_GROUPS.map((group) => group.slug)],
  ...ARCHITECTURE_GROUPS.map((group) => [group.slug, group.members]),
  ["zudo-wind/index", WIND_ENTRY_SLUGS],
  ["zudo-wind/learn", WIND_LEARN_SLUGS],
  ...WIND_UTILITY_GROUPS.map((group) => [group.slug, group.members]),
]);

export function syncNavigationIndexes(check = false) {
  let stale = false;
  for (const locale of ["docs", "docs-ja"])
    for (const [slug, members] of NAVIGATION_INDEX_MEMBERS) {
      const file = fileURLToPath(new URL(`../src/content/${locale}/${slug}.mdx`, import.meta.url));
      const source = readFileSync(file, "utf8");
      const next = source
        .replace(/^import \{[^\n]+\} from "[^"\n]*navigation-groups\.mjs";\n\n/gm, "")
        .replace(
          /<CategoryNav categories=\{[^\n]+\} \/>/,
          `<CategoryNav categories={${JSON.stringify(members)}} />`,
        );
      if (next === source) continue;
      if (check) {
        console.error(`Stale native index membership: ${file}`);
        stale = true;
      } else writeFileSync(file, next);
    }
  return !stale;
}
if (process.argv[1] === fileURLToPath(import.meta.url))
  process.exitCode = syncNavigationIndexes(process.argv.includes("--check")) ? 0 : 1;
