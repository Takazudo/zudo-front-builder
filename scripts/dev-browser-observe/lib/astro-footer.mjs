export const ASTRO_FOOTER = "Same content. Same design. Three build systems.";
export function astroFooterPredicate(value) {
  const selector = ".site-footer > p";
  return {
    kind: "all",
    of: [
      { kind: "count", selector, equals: 1 },
      { kind: "text", selector, equals: value },
    ],
  };
}
export function matchesAstroFooter(actual, value) {
  return Array.isArray(actual) && actual.length === 2 && actual[0] === 1 && actual[1] === value;
}
