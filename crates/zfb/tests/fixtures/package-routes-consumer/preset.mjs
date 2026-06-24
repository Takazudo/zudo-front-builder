/**
 * Simulates a zudo-doc-style preset: injects one static route and one
 * dynamic route (with literal paths()) alongside the consumer's own pages/.
 *
 * Sharp edge: package route pages must NOT use *.module.css or
 * import.meta.glob — source transforms are skipped for package route pages.
 * Plain TSX + relative TS imports + node_modules imports only.
 */
export default {
  name: "consumer-preset",
  setup({ injectRoute }) {
    // Static route: a fixed URL that the package always provides.
    injectRoute("/preset-about", "./pkg/about.tsx");
    // Dynamic route: one page per documentation entry, enumerated via
    // literal paths() (no getCollection needed for this fixture).
    injectRoute("/preset-docs/[slug]", "./pkg/docs.tsx");
  },
};
