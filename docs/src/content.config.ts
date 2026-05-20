import { defineCollection } from "astro:content";
import { z } from "astro/zod";
import { glob } from "astro/loaders";
import { settings } from "./config/settings";

// Recipes catalog metadata — see issue #348 / research/348-recipes-catalog.md.
// Required when a doc declares itself a recipe (top-level `recipe:` key);
// the strict() ensures stale fields cannot be silently tolerated.
const recipeSchema = z
  .object({
    // Project this recipe was first authored in / extracted from.
    provenance: z.string(),
    // YYYY-MM. The month the author last manually re-ran the recipe end-to-end.
    verified: z.string().regex(/^\d{4}-\d{2}$/, "verified must be YYYY-MM"),
    // npm-style semver range like ">=0.1 <0.2" — not a single version.
    applies_to_zfb_version: z.string(),
    // One short paragraph naming the costs and the alternatives considered.
    tradeoffs: z.string(),
    // Known break conditions; readers should scan this before adopting.
    breaks_if: z.array(z.string()).min(1),
    // Lifecycle marker. "seed" = drafted without end-to-end re-verification.
    status: z.enum(["seed", "verified", "deprecated"]).optional(),
  })
  .strict();

const docsSchema = z.object({
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
  standalone: z.boolean().optional(),
  slug: z.string().optional(),
  generated: z.boolean().optional(),
  // Recipes catalog: when present, all sub-fields are required. See #348.
  recipe: recipeSchema.optional(),
});

const docs = defineCollection({
  loader: glob({ pattern: "**/*.{md,mdx}", base: `./${settings.docsDir}` }),
  schema: docsSchema,
});

const localeCollections: Record<string, ReturnType<typeof defineCollection>> = {};
for (const [code, config] of Object.entries(settings.locales)) {
  localeCollections[`docs-${code}`] = defineCollection({
    loader: glob({ pattern: "**/*.{md,mdx}", base: `./${config.dir}` }),
    schema: docsSchema,
  });
}

const versionCollections: Record<string, ReturnType<typeof defineCollection>> = {};
if (settings.versions) {
  for (const version of settings.versions) {
    versionCollections[`docs-v-${version.slug}`] = defineCollection({
      loader: glob({ pattern: "**/*.{md,mdx}", base: `./${version.docsDir}` }),
      schema: docsSchema,
    });
    if (version.locales) {
      for (const [code, config] of Object.entries(version.locales)) {
        versionCollections[`docs-v-${version.slug}-${code}`] = defineCollection({
          loader: glob({ pattern: "**/*.{md,mdx}", base: `./${config.dir}` }),
          schema: docsSchema,
        });
      }
    }
  }
}

export const collections = { docs, ...localeCollections, ...versionCollections };
