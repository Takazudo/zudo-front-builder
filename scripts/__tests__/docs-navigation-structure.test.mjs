import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import {
  ARCHITECTURE_GROUPS,
  BUILD_GROUPS,
  DEPLOY_SLUGS,
  WIND_UTILITY_GROUPS,
  WIND_LEARN_SLUGS,
  WIND_ENTRY_SLUGS,
  COMPATIBILITY_BUCKETS,
  DOCS_CATEGORY_ORDER,
} from "../../docs/src/config/navigation-groups.mjs";
import { WIND_REFERENCE_FAMILIES } from "../../docs/scripts/wind-reference-families.mjs";
import {
  buildEditorialForest,
  buildWindEditorialForest,
  canonicalNavSection,
  editorialForestFor,
  resolveNavigationScope,
  sidebarDefinitionFor,
} from "../../docs/src/config/navigation-scope.mjs";
import { DOCS_ROUTE_MIGRATIONS } from "../../docs/src/config/route-migrations.mjs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

// The docs importer owns the published zudo-doc graph (the root workspace does
// not resolve it), so resolve the public subpath export from docs/package.json.
const docsRequire = createRequire(new URL("../../docs/package.json", import.meta.url));
// A dynamic import is unavoidable here (resolution goes through docs/); a missing
// docs install must fail loudly with the fix, not as an opaque resolver error.
async function importSidebarUtils() {
  try {
    return await import(
      pathToFileURL(docsRequire.resolve("@takazudo/zudo-doc/sidebar-utils")).href
    );
  } catch (error) {
    throw new Error(
      "@takazudo/zudo-doc/sidebar-utils is not resolvable from docs/package.json; the docs " +
        "workspace must be installed (run `pnpm install` at the repo root): " +
        (error?.message ?? error),
      { cause: error },
    );
  }
}
const {
  broaderSidebarScope,
  buildSidebarNavigation,
  indexSidebarOccurrences,
  sidebarBaselineOpen,
  sidebarInitialExpansion,
  sidebarScopeNodes,
} = await importSidebarUtils();

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
    const { syncNavigationIndexes, materializeNavigationIndex } =
      await import("../../docs/scripts/sync-navigation-indexes.mjs");
    expect(syncNavigationIndexes(true)).toBe(true);
    expect(() => materializeNavigationIndex("No index", [])).toThrow("exactly one");
    expect(() =>
      materializeNavigationIndex(
        "<CategoryNav categories={[]} /><CategoryNav categories={[]} />",
        [],
      ),
    ).toThrow("exactly one");
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
  function nativeWindTree() {
    // URL-shaped native tree: every utility page is a URL child of utilities.
    const utilities = WIND_UTILITY_GROUPS.flatMap((group) => [group.slug, ...group.members]);
    return [
      node("getting-started", [node("getting-started/introduction")]),
      node("zudo-wind", [
        node("zudo-wind/learn"),
        ...WIND_LEARN_SLUGS.map((slug) => node(slug)),
        node(
          "zudo-wind/utilities",
          utilities.map((slug) =>
            slug === "zudo-wind/utilities/background"
              ? node(slug, [node("zudo-wind/utilities/background/unassigned")])
              : node(slug),
          ),
        ),
        node("zudo-wind/compatibility", [node("zudo-wind/compatibility/nested")]),
        node("zudo-wind/coming-from-tailwind"),
      ]),
      node("playground", [node("playground/design-system")]),
    ];
  }
  // The same scope definition and forest docs/src/navigation-chrome.tsx feeds
  // the paired Header/Sidebar (asserted below), resolved by real zudo-doc.
  function windNavigation(slug, tree = nativeWindTree()) {
    const scope = resolveNavigationScope(slug, "zudo-wind");
    return buildSidebarNavigation(
      [],
      "ja",
      "zudo-wind",
      undefined,
      { "zudo-wind": sidebarDefinitionFor(scope) },
      () => editorialForestFor(scope, tree),
      [],
    );
  }
  it("configures the paired Header/Sidebar from the shared scope definition", () => {
    const chrome = readFileSync("docs/src/navigation-chrome.tsx", "utf8");
    expect(chrome).toContain("[scope.navSection]: sidebarDefinitionFor(scope)");
    expect(chrome).toContain("editorialForestFor(scope, routeCtx.buildNavTree(...args))");
    expect(chrome).toContain('const key = `${locale}:${scope?.id ?? "native"}`;');
    // Deploy gets its tree from its scope like every other section, not a flat override.
    expect(chrome).not.toContain("DEPLOY_SLUGS");
    expect(sidebarDefinitionFor(resolveNavigationScope("zudo-wind/learn", "zudo-wind"))).toEqual({
      items: [{ type: "autogenerated" }],
      initialPath: [0, 0],
      initialPathMode: "node",
    });
  });
  it("rebuilds the complete editorial forest with native metadata and no mutation", () => {
    const tree = nativeWindTree();
    const before = JSON.stringify(tree);
    const [root] = buildWindEditorialForest(tree);
    expect(JSON.stringify(tree)).toBe(before);
    expect(root.slug).toBe("zudo-wind");
    expect(root.children.map((child) => child.slug)).toEqual(WIND_ENTRY_SLUGS);
    const [learn, utilities, compatibility, designSystem] = root.children;
    expect(learn.children.map((child) => child.slug)).toEqual(WIND_LEARN_SLUGS);
    expect(utilities.children.map((child) => child.slug)).toEqual(
      WIND_UTILITY_GROUPS.map((group) => group.slug),
    );
    for (const [index, group] of WIND_UTILITY_GROUPS.entries())
      expect(utilities.children[index].children.map((child) => child.slug)).toEqual(group.members);
    // Members without editorial membership are leaves: native URL descendants
    // (unassigned pages) do not leak in, matching the pre-06R local views.
    expect(compatibility.children).toEqual([]);
    const surfaces = utilities.children.find((child) => child.slug.endsWith("/surfaces-borders"));
    const background = surfaces.children.find((child) => child.slug.endsWith("/background"));
    expect(background.children).toEqual([]);
    const listed = JSON.stringify(root);
    expect(listed).not.toContain("compatibility/nested");
    expect(listed).not.toContain("background/unassigned");
    expect(designSystem.href).toBe("/ja/docs/playground/design-system/");
    const visit = (nodes) => nodes.flatMap((current) => [current, ...visit(current.children)]);
    for (const current of visit([root])) {
      expect(current.href).toBe(`/ja/docs/${current.slug}/`);
      expect(current.metadata.retained).toBe(true);
    }
    expect(() => buildWindEditorialForest([])).toThrow("requires routed node zudo-wind");
  });
  it("keeps editorial ancestry Background → Surfaces & borders → Utility reference → Wind", () => {
    const { navigation } = windNavigation("zudo-wind/utilities/background");
    const occurrences = indexSidebarOccurrences(navigation.roots);
    expect(navigation.roots.map((root) => root.slug)).toEqual(["zudo-wind"]);
    const background = [...occurrences].find(
      ([, current]) => current.slug === "zudo-wind/utilities/background",
    );
    const ancestry = [];
    for (let id = navigation.parents[background[0]]; id; id = navigation.parents[id])
      ancestry.push(occurrences.get(id).slug);
    expect(ancestry).toEqual([
      "zudo-wind/utilities/surfaces-borders",
      "zudo-wind/utilities",
      "zudo-wind",
    ]);
    // The Wind cross-link is a distinct occurrence; the URL tree keeps Playground as its parent.
    const designSystem = [...occurrences].filter(
      ([, current]) => current.slug === "playground/design-system",
    );
    expect(designSystem).toHaveLength(1);
    expect(occurrences.get(navigation.parents[designSystem[0][0]]).slug).toBe("zudo-wind");
    expect(nativeWindTree()[2].children.map((child) => child.slug)).toEqual([
      "playground/design-system",
    ]);
  });
  it("starts each configured view at its linked root and broadens to its real editorial parent", () => {
    const cases = [
      [
        "zudo-wind/utilities/background",
        "zudo-wind/utilities/surfaces-borders",
        ["zudo-wind/utilities", "zudo-wind"],
      ],
      [
        "zudo-wind/utilities/surfaces-borders",
        "zudo-wind/utilities/surfaces-borders",
        ["zudo-wind/utilities", "zudo-wind"],
      ],
      ["zudo-wind/utilities", "zudo-wind/utilities", ["zudo-wind"]],
      ["zudo-wind/learn", "zudo-wind/learn", ["zudo-wind"]],
      [WIND_LEARN_SLUGS[3], "zudo-wind/learn", ["zudo-wind"]],
      ["zudo-wind", "zudo-wind", []],
      ["zudo-wind/compatibility", "zudo-wind", []],
      ["zudo-wind/coming-from-tailwind", "zudo-wind", []],
    ];
    for (const group of WIND_UTILITY_GROUPS)
      cases.push([group.members[0], group.slug, ["zudo-wind/utilities", "zudo-wind"]]);
    const ids = new Set();
    for (const [slug, localRoot, broadens] of cases) {
      const { nodes, navigation } = windNavigation(slug);
      ids.add(navigation.id);
      const occurrences = indexSidebarOccurrences(navigation.roots);
      const scope = resolveNavigationScope(slug, "zudo-wind");
      expect(scope.rootSlug).toBe(localRoot);
      // The linked root appears exactly once, followed by its ordered members.
      expect(nodes).toHaveLength(1);
      const [root] = nodes;
      expect(root.slug).toBe(localRoot);
      expect(root.hasPage).toBe(true);
      expect(root.href).toBe(`/ja/docs/${localRoot}/`);
      expect(root.children.map((current) => current.slug)).toEqual(scope.memberSlugs);
      expect(occurrences.get(root.occurrenceId)).toBe(root);
      // localParentId is the root occurrence's real parent; Wind has none.
      expect(navigation.localParentId).toBe(navigation.parents[root.occurrenceId] ?? undefined);
      if (localRoot === "zudo-wind") expect(navigation).not.toHaveProperty("localParentId");
      // A selected scope equal to the baseline renders the baseline itself.
      expect(sidebarScopeNodes(navigation, nodes, root.occurrenceId)).toEqual(nodes);
      const actual = [];
      for (
        let selected = broaderSidebarScope(navigation, nodes, null);
        selected !== null;
        selected = broaderSidebarScope(navigation, nodes, selected)
      )
        actual.push(occurrences.get(selected).slug);
      expect(actual).toEqual(broadens);
      if (actual.length) expect(actual.at(-1)).toBe("zudo-wind");
    }
    // Coming from Tailwind keeps Wind scope without joining its four entries.
    expect(WIND_ENTRY_SLUGS).not.toContain("zudo-wind/coming-from-tailwind");
    // One editorial context: stored scope survives cross-branch Wind navigation.
    expect(ids.size).toBe(1);
  });
  it("keeps Wind local views compact through the native nearest-index expansion default", () => {
    for (const locale of ["docs", "docs-ja"])
      expect(readFileSync(`docs/src/content/${locale}/zudo-wind/index.mdx`, "utf8")).toMatch(
        /^---\n[\s\S]*?^sidebar_initial_expansion: collapsed$[\s\S]*?^---$/m,
      );
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
  // URL-shaped native tree for Architecture, Build with zfb and Deploy & Embed:
  // group landing pages are URL siblings of their articles, and Build/Deploy
  // members live in other sections (cross-links such as Markdown Features carry
  // their own subtrees).
  function nativeSectionTree(locale = "ja") {
    const at = (slug, children = []) => ({
      ...node(slug, children),
      label: `${locale}:${slug}`,
      href: `${locale === "ja" ? "/ja" : ""}/docs/${slug}/`,
    });
    const architecture = ARCHITECTURE_GROUPS.flatMap((group) => [group.slug, ...group.members]);
    const guides = [
      ...new Set([...BUILD_GROUPS.flatMap((group) => group.members), ...DEPLOY_SLUGS]),
    ].filter((slug) => slug.startsWith("guides/"));
    return [
      at("getting-started", [
        at("getting-started/introduction"),
        at("getting-started/your-first-site"),
      ]),
      at(
        "build-with-zfb",
        BUILD_GROUPS.map((group) => at(group.slug)),
      ),
      at("zudo-wind", [at("zudo-wind/learn"), at("zudo-wind/utilities")]),
      at("api", [at("api/cli")]),
      at(
        "architecture",
        architecture.map((slug) => at(slug)),
      ),
      at(
        "guides",
        guides.map((slug) => at(slug)),
      ),
      at("markdown-features", [at("markdown-features/directives")]),
      at("deploy-embed"),
    ];
  }
  function sectionNavigation(slug, navSection, locale = "ja", tree = nativeSectionTree(locale)) {
    const scope = resolveNavigationScope(slug, navSection);
    return buildSidebarNavigation(
      [{ slug: "architecture/index", data: { sidebar_initial_expansion: "collapsed" } }],
      locale,
      navSection,
      undefined,
      { [navSection]: sidebarDefinitionFor(scope) },
      () => editorialForestFor(scope, tree),
      [],
    );
  }
  const walk = (nodes) => nodes.flatMap((current) => [current, ...walk(current.children)]);
  it("rebuilds Architecture from its real landing page with four linked groups", () => {
    for (const locale of ["en", "ja"]) {
      const tree = nativeSectionTree(locale);
      const before = JSON.stringify(tree);
      const [root] = buildEditorialForest(tree, "architecture");
      expect(JSON.stringify(tree)).toBe(before);
      expect(root.slug).toBe("architecture");
      expect(root.label).toBe(`${locale}:architecture`);
      expect(root.children.map((group) => group.slug)).toEqual(
        ARCHITECTURE_GROUPS.map((group) => group.slug),
      );
      expect(root.children.map((group) => group.children.length)).toEqual([6, 7, 7, 8]);
      for (const [index, group] of ARCHITECTURE_GROUPS.entries()) {
        const row = root.children[index];
        expect(row.hasPage).toBe(true);
        expect(row.href).toBe(`${locale === "ja" ? "/ja" : ""}/docs/${group.slug}/`);
        expect(row.label).toBe(`${locale}:${group.slug}`);
        // No repeated own-index child: the group row is the landing page link.
        expect(row.children.map((child) => child.slug)).toEqual(group.members);
        for (const child of row.children) expect(child.children).toEqual([]);
      }
      const slugs = walk([root]).map((current) => current.slug);
      expect(slugs).toHaveLength(1 + 4 + 28);
      expect(new Set(slugs).size).toBe(slugs.length);
      for (const current of walk([root])) expect(current.metadata.retained).toBe(true);
    }
    expect(() => buildEditorialForest([], "architecture")).toThrow(
      "requires routed node architecture",
    );
    const withoutGroupPage = nativeSectionTree();
    const architectureNode = withoutGroupPage.find((current) => current.slug === "architecture");
    architectureNode.children = architectureNode.children.filter(
      (current) => current.slug !== "architecture/rendering",
    );
    expect(() => buildEditorialForest(withoutGroupPage, "architecture")).toThrow(
      "requires routed node architecture/rendering",
    );
    expect(() => buildEditorialForest([], "guides")).toThrow("No editorial navigation for guides");
  });
  it("shows Architecture's four groups as peers, rooted at the real landing page", () => {
    const ids = new Set();
    const articles = ARCHITECTURE_GROUPS.flatMap((group) => [group.slug, ...group.members]);
    for (const slug of articles) {
      const { nodes, navigation } = sectionNavigation(slug, "architecture");
      ids.add(navigation.id);
      expect(nodes.map((current) => current.slug)).toEqual(
        ARCHITECTURE_GROUPS.map((group) => group.slug),
      );
      expect(navigation.roots.map((current) => current.slug)).toEqual(["architecture"]);
      // Native Broaden reaches the real Architecture landing page, then stops.
      const occurrences = indexSidebarOccurrences(navigation.roots);
      const broader = broaderSidebarScope(navigation, nodes, null);
      expect(occurrences.get(broader).slug).toBe("architecture");
      expect(sidebarScopeNodes(navigation, nodes, broader)).toEqual(navigation.roots);
      expect(broaderSidebarScope(navigation, nodes, broader)).toBeNull();
      expect(navigation.localParentId).toBe(broader);
      expect(navigation.initialExpansion).toEqual({ architecture: "collapsed" });
      // Fresh state reveals only the group holding the current page.
      const mode = sidebarInitialExpansion(navigation, slug);
      expect(mode).toBe("collapsed");
      expect(nodes.map((group) => sidebarBaselineOpen(group, mode, slug))).toEqual(
        ARCHITECTURE_GROUPS.map((group) => group.slug === slug || group.members.includes(slug)),
      );
    }
    // The overview is the native node-mode root view: its row once, then the groups.
    const overview = sectionNavigation("architecture", "architecture");
    expect(overview.nodes).toHaveLength(1);
    expect(overview.nodes[0].slug).toBe("architecture");
    expect(overview.nodes[0].children).toHaveLength(4);
    expect(overview.navigation).not.toHaveProperty("localParentId");
    expect(broaderSidebarScope(overview.navigation, overview.nodes, null)).toBeNull();
    const overviewMode = sidebarInitialExpansion(overview.navigation, "architecture");
    expect(sidebarBaselineOpen(overview.nodes[0], overviewMode, "architecture")).toBe(true);
    for (const group of overview.nodes[0].children)
      expect(sidebarBaselineOpen(group, overviewMode, "architecture")).toBe(false);
    // One editorial context across the overview and four-group baselines.
    ids.add(overview.navigation.id);
    expect(ids.size).toBe(1);
  });
  it("gives Build with zfb six linked groups with intentional repeats and leaf cross-links", () => {
    for (const locale of ["en", "ja"]) {
      const tree = nativeSectionTree(locale);
      const before = JSON.stringify(tree);
      for (const slug of [
        "build-with-zfb",
        "build-with-zfb/pages-routes",
        "guides/troubleshooting",
      ]) {
        const { nodes, navigation } = sectionNavigation(slug, "build-with-zfb", locale, tree);
        expect(JSON.stringify(tree)).toBe(before);
        expect(nodes).toHaveLength(1);
        const [root] = nodes;
        expect(root.slug).toBe("build-with-zfb");
        expect(root.href).toBe(`${locale === "ja" ? "/ja" : ""}/docs/build-with-zfb/`);
        expect(root.children.map((group) => group.slug)).toEqual(
          BUILD_GROUPS.map((group) => group.slug),
        );
        for (const [index, group] of BUILD_GROUPS.entries()) {
          expect(root.children[index].hasPage).toBe(true);
          expect(root.children[index].children.map((child) => child.slug)).toEqual(group.members);
        }
        const occurrences = walk(nodes).map((current) => current.slug);
        expect(occurrences.filter((current) => current === "guides/troubleshooting")).toHaveLength(
          2,
        );
        for (const group of BUILD_GROUPS)
          expect(occurrences.filter((current) => current === group.slug)).toHaveLength(1);
        // Cross-links stay leaves instead of importing their sections.
        for (const leaf of ["markdown-features", "zudo-wind", "api/cli", "architecture/routing"])
          expect(walk(nodes).find((current) => current.slug === leaf).children).toEqual([]);
        expect(occurrences).not.toContain("markdown-features/directives");
        expect(occurrences).not.toContain("zudo-wind/learn");
        expect(broaderSidebarScope(navigation, nodes, null)).toBeNull();
      }
    }
  });
  it("rebuilds Deploy & Embed from its real landing page with four ordered leaves", () => {
    const owned = DEPLOY_SLUGS.filter((slug) => slug.startsWith("guides/"));
    expect(owned).toHaveLength(3);
    for (const locale of ["en", "ja"]) {
      const prefix = locale === "ja" ? "/ja" : "";
      const tree = nativeSectionTree(locale);
      const before = JSON.stringify(tree);
      const [forestRoot] = buildEditorialForest(tree, "deploy-embed");
      expect(forestRoot.slug).toBe("deploy-embed");
      expect(forestRoot.children.map((child) => child.slug)).toEqual(DEPLOY_SLUGS);
      const ids = new Set();
      for (const slug of ["deploy-embed", ...owned]) {
        const scope = resolveNavigationScope(slug, canonicalNavSection(slug, "deploy-embed"));
        expect(scope.id).toBe("deploy");
        expect(sidebarDefinitionFor(scope)).toEqual({
          items: [{ type: "autogenerated" }],
          initialPath: [0],
          initialPathMode: "node",
        });
        // The chrome canonicalizes first: a guide reached from Guides still uses Deploy.
        const navSection = canonicalNavSection(slug, slug === "deploy-embed" ? slug : "guides");
        expect(navSection).toBe("deploy-embed");
        const { nodes, navigation } = sectionNavigation(slug, navSection, locale, tree);
        expect(JSON.stringify(tree)).toBe(before);
        ids.add(navigation.id);
        // One page-backed root, then the four members as leaves; the landing
        // page is not repeated as an Overview child.
        expect(nodes).toHaveLength(1);
        const [root] = nodes;
        expect(root.slug).toBe("deploy-embed");
        expect(root.hasPage).toBe(true);
        expect(root.href).toBe(`${prefix}/docs/deploy-embed/`);
        expect(root.label).toBe(`${locale}:deploy-embed`);
        expect(root.children.map((child) => child.slug)).toEqual(DEPLOY_SLUGS);
        for (const child of root.children) {
          expect(child.children).toEqual([]);
          expect(child.href).toBe(`${prefix}/docs/${child.slug}/`);
          expect(child.label).toBe(`${locale}:${child.slug}`);
        }
        const slugs = walk(nodes).map((current) => current.slug);
        expect(slugs.filter((current) => current === "deploy-embed")).toHaveLength(1);
        expect(slugs).toHaveLength(1 + DEPLOY_SLUGS.length);
        for (const current of walk(nodes)) expect(current.metadata.retained).toBe(true);
        // The real Deploy root is the highest scope: Broaden has nowhere to go.
        expect(navigation.roots.map((current) => current.slug)).toEqual(["deploy-embed"]);
        expect(navigation).not.toHaveProperty("localParentId");
        expect(broaderSidebarScope(navigation, nodes, null)).toBeNull();
        // Fresh arrival reveals the root and its children.
        const mode = sidebarInitialExpansion(navigation, slug);
        expect(sidebarBaselineOpen(root, mode, slug)).toBe(true);
      }
      // One editorial context for the landing page and its three owned guides.
      expect(ids.size).toBe(1);
    }
    const withoutGuide = nativeSectionTree();
    const guidesNode = withoutGuide.find((current) => current.slug === "guides");
    guidesNode.children = guidesNode.children.filter(
      (current) => current.slug !== "guides/embed-as-library",
    );
    expect(() => buildEditorialForest(withoutGuide, "deploy-embed")).toThrow(
      "requires routed node guides/embed-as-library",
    );
  });
  it("keeps Three ways to run zfb listed in Deploy but owned by Architecture", () => {
    expect(DEPLOY_SLUGS[0]).toBe("architecture/three-ways-to-run-zfb");
    for (const linked of ["deploy-embed", "architecture", undefined]) {
      const section = canonicalNavSection("architecture/three-ways-to-run-zfb", linked);
      expect(section).toBe("architecture");
      expect(resolveNavigationScope("architecture/three-ways-to-run-zfb", section).id).toBe(
        "architecture",
      );
    }
    // Ownership never follows the link that reached the page.
    expect(
      resolveNavigationScope("architecture/three-ways-to-run-zfb", "deploy-embed"),
    ).toBeUndefined();
    for (const slug of DEPLOY_SLUGS.filter((current) => current.startsWith("guides/")))
      for (const linked of ["deploy-embed", "guides", "build-with-zfb", undefined])
        expect(canonicalNavSection(slug, linked)).toBe("deploy-embed");
    // Unknown Deploy pages fall back to native navigation.
    expect(resolveNavigationScope("deploy-embed/future", "deploy-embed")).toBeUndefined();
    expect(resolveNavigationScope("guides/troubleshooting", "deploy-embed")).toBeUndefined();
  });
  it("keeps Architecture canonical for every Architecture route", () => {
    for (const group of ARCHITECTURE_GROUPS)
      for (const slug of [group.slug, ...group.members])
        for (const linked of ["architecture", "build-with-zfb", "deploy-embed", undefined])
          expect(canonicalNavSection(slug, linked)).toBe("architecture");
    expect(canonicalNavSection("architecture", "architecture")).toBe("architecture");
    expect(canonicalNavSection("architecture/routing", "build-with-zfb")).toBe("architecture");
    expect(canonicalNavSection("architecture/three-ways-to-run-zfb", "deploy-embed")).toBe(
      "architecture",
    );
    // Unrelated Guide/Deploy handling is preserved.
    expect(canonicalNavSection("guides/ssr-and-cloudflare-bindings", "guides")).toBe(
      "deploy-embed",
    );
    expect(canonicalNavSection("guides/troubleshooting", "guides")).toBe("build-with-zfb");
    expect(canonicalNavSection("guides/client-scripts", "guides")).toBe("build-with-zfb");
    expect(canonicalNavSection("api/cli", "api")).toBe("api");
    expect(canonicalNavSection("getting-started/your-first-site", "getting-started")).toBe(
      "getting-started",
    );
    expect(canonicalNavSection("markdown-features", "markdown-features")).toBe("markdown-features");
  });
  it("gives every distinct editorial forest and baseline its own chrome cache key", () => {
    const cases = [
      ["architecture", "architecture"],
      ["architecture/routing", "architecture"],
      ["build-with-zfb", "build-with-zfb"],
      ["guides/troubleshooting", "build-with-zfb"],
      ["zudo-wind", "zudo-wind"],
      ["zudo-wind/learn", "zudo-wind"],
      ["deploy-embed", "deploy-embed"],
      ["guides/ssr-and-cloudflare-bindings", "guides"],
      ["guides/embed-as-library", "deploy-embed"],
      ["architecture/three-ways-to-run-zfb", "deploy-embed"],
      ["getting-started/introduction", "getting-started"],
    ];
    const byKey = new Map();
    for (const [slug, navSection] of cases) {
      const scope = resolveNavigationScope(slug, canonicalNavSection(slug, navSection));
      const identity = scope
        ? JSON.stringify([scope.navSection, scope.editorialRootSlug, sidebarDefinitionFor(scope)])
        : "native";
      const key = scope?.id ?? "native";
      if (byKey.has(key)) expect(byKey.get(key)).toBe(identity);
      byKey.set(key, identity);
    }
    expect(new Set(byKey.values()).size).toBe(byKey.size);
    expect([...byKey.keys()]).toEqual([
      "architecture-overview",
      "architecture",
      "build",
      "wind",
      "wind-learn",
      "deploy",
      "native",
    ]);
    // Unknown pages fall back to native (unscoped) navigation.
    expect(resolveNavigationScope("architecture/future", "architecture")).toBeUndefined();
    expect(resolveNavigationScope("build-with-zfb/future", "build-with-zfb")).toBeUndefined();
  });
  it("keeps Architecture views compact through the native nearest-index expansion default", () => {
    for (const locale of ["docs", "docs-ja"])
      expect(readFileSync(`docs/src/content/${locale}/architecture/index.mdx`, "utf8")).toMatch(
        /^---\n[\s\S]*?^sidebar_initial_expansion: collapsed$[\s\S]*?^---$/m,
      );
  });
  it("resolves every Architecture, Build with zfb and Deploy page to an editorial scope", () => {
    for (const locale of ["docs", "docs-ja"])
      for (const section of ["architecture", "build-with-zfb", "deploy-embed"])
        for (const file of readdirSync(`docs/src/content/${locale}/${section}`)) {
          const slug = `${section}/${file.replace(/\.mdx$/, "")}`.replace(/\/index$/, "");
          expect(resolveNavigationScope(slug, section), `${locale}/${slug}`).toBeDefined();
        }
    const scopes = [
      ...WIND_LEARN_SLUGS,
      ...WIND_UTILITY_GROUPS.map((group) => group.slug),
      "zudo-wind",
      "zudo-wind/utilities",
    ].map((slug) => resolveNavigationScope(slug, "zudo-wind"));
    scopes.push(
      resolveNavigationScope("architecture", "architecture"),
      resolveNavigationScope("architecture/routing", "architecture"),
      resolveNavigationScope("build-with-zfb", "build-with-zfb"),
      resolveNavigationScope("deploy-embed", "deploy-embed"),
      resolveNavigationScope("guides/desktop-deployment", "deploy-embed"),
    );
    const byId = new Map(scopes.map((scope) => [scope.id, scope]));
    for (const scope of scopes) expect(byId.get(scope.id)).toBe(scope);
    expect(byId.size).toBe(new Set(scopes).size);
  });
});
