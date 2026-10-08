import { WIND_ENTRY_SLUGS, WIND_LEARN_SLUGS, WIND_UTILITY_GROUPS } from "./navigation-groups.mjs";

const entryScope = {
  id: "wind",
  navSection: "zudo-wind",
  rootSlug: "zudo-wind",
  memberSlugs: WIND_ENTRY_SLUGS,
};
const learnScope = {
  id: "wind-learn",
  navSection: "zudo-wind",
  rootSlug: "zudo-wind/learn",
  memberSlugs: WIND_LEARN_SLUGS,
};
const utilitiesScope = {
  id: "wind-utilities",
  navSection: "zudo-wind",
  rootSlug: "zudo-wind/utilities",
  memberSlugs: WIND_UTILITY_GROUPS.map((group) => group.slug),
};
const groupScopes = WIND_UTILITY_GROUPS.map((group) => ({
  id: group.id,
  navSection: "zudo-wind",
  rootSlug: group.slug,
  memberSlugs: group.members,
}));

export function resolveNavigationScope(currentSlug, navSection) {
  if (navSection !== "zudo-wind") return undefined;
  if (
    ["zudo-wind", "zudo-wind/compatibility", "zudo-wind/coming-from-tailwind"].includes(currentSlug)
  )
    return entryScope;
  if (currentSlug === learnScope.rootSlug || WIND_LEARN_SLUGS.includes(currentSlug))
    return learnScope;
  if (currentSlug === utilitiesScope.rootSlug) return utilitiesScope;
  return groupScopes.find(
    (scope) => scope.rootSlug === currentSlug || scope.memberSlugs.includes(currentSlug),
  );
}

export function selectNavigationTree(tree, scope) {
  const nodes = new Map();
  function visit(children) {
    for (const node of children) {
      nodes.set(node.slug, node);
      visit(node.children);
    }
  }
  visit(tree);
  function required(slug) {
    const node = nodes.get(slug);
    if (!node?.hasPage || !node.href)
      throw new Error(`Navigation scope ${scope.id} requires routed node ${slug}`);
    return node;
  }
  return [
    {
      ...required(scope.rootSlug),
      children: scope.memberSlugs.map((slug) => ({ ...required(slug), children: [] })),
    },
  ];
}
