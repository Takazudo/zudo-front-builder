import { WIND_ENTRY_SLUGS, WIND_LEARN_SLUGS, WIND_UTILITY_GROUPS } from "./navigation-groups.mjs";

const WIND_ROOT_SLUG = "zudo-wind";

// Editorial parent → ordered members. Members are often URL siblings of their
// parent (`utilities/background` under `utilities/surfaces-borders`), so the
// hierarchy comes from this shared membership source, never from URL segments.
const WIND_EDITORIAL_MEMBERS = new Map([
  [WIND_ROOT_SLUG, WIND_ENTRY_SLUGS],
  ["zudo-wind/learn", WIND_LEARN_SLUGS],
  ["zudo-wind/utilities", WIND_UTILITY_GROUPS.map((group) => group.slug)],
  ...WIND_UTILITY_GROUPS.map((group) => [group.slug, group.members]),
]);

function windScope(id, rootSlug) {
  return {
    id,
    navSection: WIND_ROOT_SLUG,
    rootSlug,
    memberSlugs: WIND_EDITORIAL_MEMBERS.get(rootSlug),
    path: editorialPath(rootSlug),
  };
}

// Occurrence indexes from the Wind root to `slug` along the editorial tree.
function editorialPath(slug) {
  if (slug === WIND_ROOT_SLUG) return [];
  for (const [parent, members] of WIND_EDITORIAL_MEMBERS) {
    const index = members.indexOf(slug);
    if (index >= 0) return [...editorialPath(parent), index];
  }
  throw new Error(`No editorial Wind parent for ${slug}`);
}

const entryScope = windScope("wind", WIND_ROOT_SLUG);
const learnScope = windScope("wind-learn", "zudo-wind/learn");
const utilitiesScope = windScope("wind-utilities", "zudo-wind/utilities");
const groupScopes = WIND_UTILITY_GROUPS.map((group) => windScope(group.id, group.slug));

export function resolveNavigationScope(currentSlug, navSection) {
  if (navSection !== WIND_ROOT_SLUG) return undefined;
  // Coming from Tailwind keeps the entry view without being one of its menu members.
  if (
    [WIND_ROOT_SLUG, "zudo-wind/compatibility", "zudo-wind/coming-from-tailwind"].includes(
      currentSlug,
    )
  )
    return entryScope;
  if (currentSlug === learnScope.rootSlug || WIND_LEARN_SLUGS.includes(currentSlug))
    return learnScope;
  if (currentSlug === utilitiesScope.rootSlug) return utilitiesScope;
  return groupScopes.find(
    (scope) => scope.rootSlug === currentSlug || scope.memberSlugs.includes(currentSlug),
  );
}

/**
 * Rebuild the complete Wind editorial forest from the native nav tree.
 * Every node keeps its native label, href and metadata; only `children` is
 * replaced along configured editorial edges. Nodes outside the membership map
 * keep their native descendants. The input tree is not mutated.
 */
export function buildWindEditorialForest(tree) {
  const nodes = new Map();
  function visit(children) {
    for (const node of children) {
      nodes.set(node.slug, node);
      visit(node.children);
    }
  }
  visit(tree);
  function build(slug) {
    const node = nodes.get(slug);
    if (!node?.hasPage || !node.href)
      throw new Error(`Wind editorial navigation requires routed node ${slug}`);
    const members = WIND_EDITORIAL_MEMBERS.get(slug);
    return { ...node, children: members ? members.map(build) : node.children };
  }
  return [build(WIND_ROOT_SLUG)];
}
