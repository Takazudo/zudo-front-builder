import { z } from "zod";

/**
 * Build the docs frontmatter zod schema.
 *
 * Returns a single `z.object(...).passthrough()` that is reused for every
 * docs collection (default + per-locale). `.passthrough()` keeps custom
 * frontmatter keys available downstream (e.g. frontmatter-preview UI).
 */
export function buildDocsSchema() {
  return z
    .object({
      title: z.string(),
      description: z.string().optional(),
      category: z.string().optional(),
      sidebar_position: z.number().optional(),
      sidebar_label: z.string().optional(),
      tags: z.array(z.string()).optional(),
      search_exclude: z.boolean().optional(),
      pagination_next: z.string().nullable().optional(),
      pagination_prev: z.string().nullable().optional(),
      draft: z.boolean().optional(),
      unlisted: z.boolean().optional(),
      hide_sidebar: z.boolean().optional(),
      hide_toc: z.boolean().optional(),
      doc_history: z.boolean().optional(),
      standalone: z.boolean().optional(),
      slug: z.string().optional(),
      generated: z.boolean().optional(),
      // Feature tier badge on Markdown Features pages (issue #877 step 6).
      tier: z.enum(["Core", "Opt-in"]).optional(),
      // Category metadata expressed as a directory index.mdx's frontmatter.
      // `category_no_page` makes the index a non-linked sidebar header excluded
      // from routes/sitemap/search; `category_sort_order` sets child sort direction.
      category_no_page: z.boolean().optional(),
      category_sort_order: z.enum(["asc", "desc"]).optional(),
    })
    .passthrough();
}
