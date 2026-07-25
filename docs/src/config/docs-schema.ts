import { z } from "zod";
import { buildDocsSchema as buildDefaultDocsSchema } from "@takazudo/zudo-doc/docs-schema";

/**
 * The package default schema plus the one key this site adds.
 *
 * `tier` drives the feature-tier badge on Markdown Features pages (#877 step
 * 6). The package default already declares every other key this site uses —
 * including `category_no_page` and `category_sort_order` — and keeps
 * `.passthrough()`, so `tier` only needs declaring to get enum validation
 * across the 46 pages that set it.
 */
export function buildDocsSchema() {
  return buildDefaultDocsSchema().extend({
    tier: z.enum(["Core", "Opt-in"]).optional(),
  });
}
