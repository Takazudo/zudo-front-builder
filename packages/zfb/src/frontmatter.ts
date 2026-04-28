// `zfb/frontmatter` — public subpath for the frontmatter parser.
//
// BCI-3: `parseFrontmatter` is promoted to a dedicated subpath so consumers
// that only need frontmatter parsing can import it without pulling in
// `zfb/content`'s Node `fs` transitive dependency chain.
//
// The implementation lives in `./content.ts` (co-located with the
// `getCollection` stub that uses it). This re-export keeps the two surfaces
// in sync without duplicating code.

export { parseFrontmatter } from "./content.js";
export type { ParsedFrontmatter } from "./content.js";
