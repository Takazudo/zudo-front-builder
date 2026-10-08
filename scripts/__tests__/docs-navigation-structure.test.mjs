import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import {
  ARCHITECTURE_GROUPS,
  WIND_UTILITY_GROUPS,
  WIND_LEARN_SLUGS,
  COMPATIBILITY_BUCKETS,
  DOCS_CATEGORY_ORDER,
} from "../../docs/src/config/navigation-groups.mjs";
import { WIND_REFERENCE_FAMILIES } from "../../docs/scripts/wind-reference-families.mjs";
import {
  resolveNavigationScope,
  selectNavigationTree,
} from "../../docs/src/config/navigation-scope.mjs";
import { DOCS_ROUTE_MIGRATIONS } from "../../docs/src/config/route-migrations.mjs";

function node(slug, children = []) {
  return {
    slug,
    label: slug,
    href: `/ja/docs/${slug}/`,
    hasPage: true,
    children,
    metadata: { retained: true },
  };
}
describe("documentation navigation contracts", () => {
  it("keeps the header order and materialized native indexes synchronized", async () => {
    const config = readFileSync("docs/zfb.config.ts", "utf8");
    expect([...config.matchAll(/categoryMatch: "([^"]+)"/g)].map((match) => match[1])).toEqual(
      DOCS_CATEGORY_ORDER,
    );
    const { syncNavigationIndexes } =
      await import("../../docs/scripts/sync-navigation-indexes.mjs");
    expect(syncNavigationIndexes(true)).toBe(true);
  });
  it("assigns every Architecture article and utility family exactly once", () => {
    const architecture = ARCHITECTURE_GROUPS.flatMap((group) => group.members);
    expect(architecture).toHaveLength(28);
    expect(new Set(architecture).size).toBe(28);
    const families = WIND_UTILITY_GROUPS.flatMap((group) =>
      group.members.map((slug) => slug.split("/").at(-1)),
    );
    expect(families.sort()).toEqual(WIND_REFERENCE_FAMILIES.map((family) => family.id).sort());
    expect(new Set(families).size).toBe(48);
    for (const group of WIND_UTILITY_GROUPS) expect(families).not.toContain(group.id);
    expect(WIND_LEARN_SLUGS).toHaveLength(8);
  });
  it("keeps one routed root, locale hrefs and metadata without mutating native nodes", () => {
    const group = WIND_UTILITY_GROUPS.find((group) => group.id === "flex-grid");
    const scope = resolveNavigationScope("zudo-wind/utilities/grid", "zudo-wind");
    const tree = [
      node("zudo-wind", [
        node("zudo-wind/utilities", [
          node(group.slug),
          ...group.members.map((slug) => node(slug, [node("nested")])),
        ]),
      ]),
    ];
    const before = JSON.stringify(tree);
    const selected = selectNavigationTree(tree, scope);
    expect(selected).toHaveLength(1);
    expect(selected[0].slug).toBe(group.slug);
    expect(selected[0].href).toBe(`/ja/docs/${group.slug}/`);
    expect(selected[0].children.map((child) => child.slug)).toEqual(group.members);
    expect(
      selected[0].children.every((child) => child.children.length === 0 && child.metadata.retained),
    ).toBe(true);
    expect(JSON.stringify(tree)).toBe(before);
    expect(() => selectNavigationTree([], scope)).toThrow("requires routed node");
  });
  it("selects all known scopes and preserves unknown-page fallback", () => {
    expect(resolveNavigationScope("zudo-wind/compatibility", "zudo-wind").rootSlug).toBe(
      "zudo-wind",
    );
    expect(resolveNavigationScope("zudo-wind/coming-from-tailwind", "zudo-wind").rootSlug).toBe(
      "zudo-wind",
    );
    for (const slug of WIND_LEARN_SLUGS)
      expect(resolveNavigationScope(slug, "zudo-wind").rootSlug).toBe("zudo-wind/learn");
    for (const group of WIND_UTILITY_GROUPS)
      for (const slug of [group.slug, ...group.members])
        expect(resolveNavigationScope(slug, "zudo-wind").rootSlug).toBe(group.slug);
    expect(resolveNavigationScope("zudo-wind/future-page", "zudo-wind")).toBeUndefined();
    expect(resolveNavigationScope("zudo-wind/utilities/grid", "guides")).toBeUndefined();
  });
  it("records explicit bilingual slash variants for every migration", () => {
    const redirects = readFileSync("docs/public/_redirects", "utf8");
    expect(COMPATIBILITY_BUCKETS).toHaveLength(23);
    for (const prefix of ["/docs", "/ja/docs"])
      for (const [from, to] of Object.entries(DOCS_ROUTE_MIGRATIONS)) {
        const [slug, anchor] = to.split("#");
        for (const slash of ["", "/"])
          expect(redirects).toContain(
            `${prefix}/${from}${slash} ${prefix}/${slug}/${anchor ? "#" + anchor : ""} 301`,
          );
      }
    expect(DOCS_ROUTE_MIGRATIONS["concepts/render-artifacts"]).toBe(
      "architecture/render-artifacts",
    );
  });
});
