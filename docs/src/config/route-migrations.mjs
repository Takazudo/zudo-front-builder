import { ARCHITECTURE_GROUPS, COMPATIBILITY_BUCKETS } from "./navigation-groups.mjs";

const originalArchitecture = new Set([
  "why-rust",
  "build-engine",
  "js-runtime",
  "framework-adapters",
]);
export const DOCS_ROUTE_MIGRATIONS = Object.freeze({
  install: "getting-started/installation",
  "install/node-free": "getting-started/installation",
  concepts: "architecture",
  ...Object.fromEntries(
    ARCHITECTURE_GROUPS.flatMap((group) => group.members)
      .filter((slug) => !originalArchitecture.has(slug.split("/").at(-1)))
      .map((slug) => [slug.replace(/^architecture\//, "concepts/"), slug]),
  ),
  ...Object.fromEntries(
    COMPATIBILITY_BUCKETS.map((bucket) => [
      `zudo-wind/compatibility/${bucket}`,
      `zudo-wind/compatibility#compat-${bucket}`,
    ]),
  ),
});

export function migratedDocSlug(slug) {
  return DOCS_ROUTE_MIGRATIONS[slug] ?? slug;
}

export function migratedBuiltRoute(route) {
  const match = route.match(/^(ja\/)?docs\/(.+)\/index\.html$/);
  if (!match) return route;
  const destination = migratedDocSlug(match[2]).split("#")[0];
  return `${match[1] ?? ""}docs/${destination}/index.html`;
}
