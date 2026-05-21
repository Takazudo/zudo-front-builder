# Research #348 — Recipes catalog discipline

Tracking issue: #358. Source issue: [#348](https://github.com/Takazudo/zudo-front-builder/issues/348).

## 1. Question — restate

zfb's "recipes over plugins" stance only pays off if the recipes catalog is well-curated. The catalog has to decide four things:

1. **Where** does it live (in-docs collection vs separate repo vs Discussions vs third-party docs surface)?
2. **What schema** do recipes carry (provenance, verified date, applies_to_zfb_version, tradeoffs, breaks_if)?
3. **What cadence** keeps recipes honest as zfb evolves (quarterly review? per-release re-verification?)?
4. **Should recipes be CI-runnable**, or is human-tested + dated metadata enough for v0?

A secondary deliverable: write 3 seed recipes from the candidate list in the issue body, with the caveat that the agent does not have access to the zzmod source repo and so cannot verify the recipes empirically — they ship marked as `status: seed`.

## 2. What was tried

- Read issue #348 in full; read the existing `docs/` content collection setup to understand authoring affordances.
- Inspected `docs/astro.config.ts` and `docs/src/content.config.ts` to confirm the routing pipeline globs the `docs` content collection and renders any MDX under `src/content/docs/...` at the matching `/docs/...` URL via `pages/docs/[...slug].astro`. **A new sub-directory under `src/content/docs/` is picked up automatically**, no astro.config or routing changes required.
- Inspected sibling content folders (`concepts/`, `guides/`) — each carries a `_category_.json` (`{ label, position }`) and per-file frontmatter `sidebar_position`. Reused that pattern unchanged.
- Read the existing worked examples in `docs/src/content/docs/concepts/plugins.mdx` for the sitemap-from-`ctx.routes` pattern and the `virtual:metadata-db` pattern; these gave concrete material for two of the three seed recipes without requiring zzmod source access.
- Decided to **extend** the existing `docsSchema` with an optional, **strict** nested `recipe` object (rather than create a separate collection or rely on review-time discipline). Rationale and tradeoffs are in §3.
- Wrote three seed recipes (the two with worked-example precedent + the OGP suffix convention), each marked with a `<Warning>` admonition and `status: seed` so adopters know the recipe has not been re-run end-to-end.
- Wrote `recipes/index.mdx` documenting the policy: what a recipe is, frontmatter contract, curation cadence, and the seed-quality warning.
- Verified the docs site still type-checks with `pnpm docs:check` (see §3 for output).

## 3. Evidence

### Location decision

**Recommendation:** `docs/src/content/docs/recipes/` (inside the existing docs site, as a sibling to `concepts/`, `guides/`, `api/`).

| Option | Verdict | Reasoning |
|---|---|---|
| `docs/src/content/docs/recipes/` (in-docs Astro collection) | **Chosen for v0** | Reuses existing routing, sidebar, search index, i18n machinery, and the `_category_.json` sidebar convention. Zero new plumbing. The recipes collection inherits the existing docs schema so we can extend it with strict recipe-specific fields without forking the loader (see schema section). |
| Separate `zfb-cookbook` repo | Rejected for v0 | Adds a second authoring surface, a second CI pipeline, and a second cross-link target. Defensible later if the catalog grows past ~40 recipes or develops its own audience, but premature now. |
| GitHub Discussions | Rejected | Curation is community-driven; quality varies. No schema enforcement. No version-pinning. The whole point of the discipline is to enforce dating and tradeoff annotation — Discussions undermines that. |
| Third-party docs surface (Cookbook-as-skill, à la `zfb-wisdom`) | Rejected for v0 | A skill is downstream of the recipes — it indexes them for AI consumption. The recipes themselves need an authoritative home first; a skill can wrap them later. |

**Risk to the existing engine docs sidebar:** sibling `_category_.json` positions verified end-to-end: Getting Started (0), Concepts (10), Guides (20), Reference (30), Architecture (40), Changelog (99), CLAUDE.md (900), Skills (902). The new `recipes/_category_.json` is set to `position: 25`, placing it between Guides and Reference — a natural reading order (concepts → guides → recipes → reference) without colliding with any existing entry. The engine docs are not crowded out.

### Schema decision and TypeScript shape

**Choice:** extend the existing `docsSchema` with an optional `recipe` Zod sub-object that is **strict** — every documented sub-field is required when the parent `recipe:` key is present, and unknown sub-fields are rejected at build time. This was a deliberate fork; I considered three options before committing:

| Schema option | Verdict | Notes |
|---|---|---|
| (a) Separate `recipes` collection with its own loader | Rejected | Would require new routing under `pages/docs/[...slug].astro` to iterate both `docs` and `recipes`. More surface; less benefit. |
| **(b) Extend `docsSchema` with strict nested `recipe` object** | **Chosen** | Real enforcement (a missing `verified` field fails the build). Recipes still flow through the existing routing/sidebar/i18n machinery. One small schema change, contained to `content.config.ts`. |
| (c) Inherit + optional + rely on review-time discipline | Rejected | Zero enforcement defeats the whole point of issue #348 — a "well-curated" catalog needs the schema to refuse stale or under-annotated recipes at build time. |

Concrete change (in `docs/src/content.config.ts`):

```ts
const recipeSchema = z
  .object({
    provenance: z.string(),
    verified: z.string().regex(/^\d{4}-\d{2}$/, "verified must be YYYY-MM"),
    applies_to_zfb_version: z.string(), // semver range string like ">=0.1 <0.2"
    tradeoffs: z.string(),
    breaks_if: z.array(z.string()).min(1),
    status: z.enum(["seed", "verified", "deprecated"]).optional(),
  })
  .strict();

const docsSchema = z.object({
  // ... existing fields ...
  recipe: recipeSchema.optional(),
});
```

Notes:

- `verified` is `YYYY-MM`, not a `Date` — month granularity is honest about how often a human actually re-runs the code; day granularity over-promises.
- `applies_to_zfb_version` is a free-form semver-range string. We do not parse it at build time (no `semver` dependency added); the contract is documented in the frontmatter and on `recipes/index.mdx`.
- `breaks_if` requires at least one entry (`.min(1)`). If you can't name one break condition, you haven't thought about the recipe long enough.
- `status: "seed"` lets the seed catalog land honestly. Non-seed recipes can omit `status` (treated as verified) once a maintainer has reproduced them.

### Seed recipes created

Three `.mdx` files under `docs/src/content/docs/recipes/`:

- `index.mdx` — policy overview, frontmatter contract, curation cadence, cross-links.
- `sitemap-from-ctx-routes.mdx` — `postBuild` plugin reading the route manifest and writing `dist/sitemap.xml`. Concrete; precedent in `concepts/plugins.mdx`.
- `ogp-suffix-convention.mdx` — sibling `<slug>__og.tsx` modules co-locating OGP metadata next to article pages. Concrete; convention used in zzmod.
- `virtual-metadata-db.mdx` — `setup` hook registering `virtual:metadata-db` whose loader walks a content tree and stringifies a typed index. Concrete; precedent in `concepts/plugins.mdx`.

Every recipe carries the full required frontmatter, a visible `<Warning>` admonition flagging seed status, and `status: seed` in its `recipe:` object.

### `_category_.json`

`docs/src/content/docs/recipes/_category_.json` registers the section in the sidebar — same shape as `concepts/_category_.json` and `guides/_category_.json`. No `astro.config.ts` changes needed; the sidebar is driven entirely by `_category_.json` files plus per-file `sidebar_position`.

### `pnpm docs:check` result

```
$ pnpm docs:check
# Result (123 files): 0 errors, 0 warnings, 3 hints
# All 3 hints are pre-existing in the repo and unrelated to this work:
#   src/components/code-block-enhancer.astro:83 — document.execCommand deprecated
#   src/components/sidebar-tree.tsx:212 — navigator.platform deprecated
#   src/integrations/claude-resources/generate.ts:260 — unused helper
```

The new collection, schema, and three seed recipes all pass Astro's type check with no new diagnostics. The recipes collection is picked up by the existing `docs` glob loader (in `content.config.ts`) automatically, and existing docs that do not declare `recipe:` continue to validate because the `recipe` field is `.optional()`.

**Schema-enforcement negative test (60-second sanity check, not a permanent test).** Removed the `verified:` field from `sitemap-from-ctx-routes.mdx` and re-ran `pnpm docs:check`. The build aborted with:

```
[InvalidContentEntryDataError] docs → recipes/sitemap-from-ctx-routes data does not match collection schema.
  recipe.verified**: **recipe.verified: Required
```

The field was restored immediately and the check went back to clean. The strict schema enforces required fields at build time as designed — a recipe missing any `recipe.*` field aborts the build.

## 4. Conclusion — recommended policy

**Location.** `docs/src/content/docs/recipes/` for v0. Re-evaluate at ~40 recipes or when the recipes' audience visibly differs from the engine docs' audience.

**Schema.** Extend `docsSchema` with a strict nested `recipe` object — required fields enforced at build time, unknown fields rejected.

**Required frontmatter fields.** `provenance`, `verified: YYYY-MM`, `applies_to_zfb_version` (semver range), `tradeoffs` (one paragraph), `breaks_if` (≥1 bullet). Optional `status: "seed" | "verified" | "deprecated"`.

**Curation cadence — explicit choice:**

- **Per-release re-verification** on every zfb minor-version bump (currently the heavier signal). The release PR walks the recipes, bumps `verified`, and updates `applies_to_zfb_version`. Recipes whose `verified` has not moved for two minor releases are flipped to `status: deprecated` in the same pass and dropped from the sidebar.
- **Quarterly low-effort sweep** for obvious rot (broken links, mis-typed paths). Anyone with write access opens a "recipe sweep" issue, fixes what they see, closes it. No assigned owner — keeps the burden honest.

This is **lighter than a per-week or per-month review** and **heavier than "trust the dates"**. The defence: a one-maintainer project cannot afford a heavier cadence, and a lighter cadence makes the `verified` field a lie.

**CI-runnable vs human-tested for v0 — explicit choice:**

- **v0: human-tested + dated metadata is enough.** Justification: every seed recipe touches build-time plugin contracts that are themselves not stable yet; pinning them to a CI matrix means re-writing the matrix every time the plugin API evolves. The maintenance cost would dwarf the catalog itself.
- The honest cost: an undated change to the plugin API can silently invalidate a recipe between releases. The mitigation is the `verified` field and the per-release re-verification cadence — `breaks_if` lists give reviewers a concrete checklist when bumping the version.
- v1+ option: spike a single recipe (e.g. the sitemap one) as an end-to-end example app under `examples/recipes/sitemap-from-ctx-routes/` with a `pnpm test` that builds the app and asserts the sitemap shape. If the per-recipe cost stays under ~30 min, expand. Otherwise stay with human-tested + dated.

**Seed-catalog honesty.** The three v0 recipes all ship with `status: seed`, a visible `<Warning>` admonition in the body, and a link to issue #348. Adopters who want a verified recipe know to wait or to open a PR.

## 5. Follow-ups

- **The two unwritten seed candidates from issue #348.** Skipped here because the agent had no zzmod source access:
  - Photo pipeline glue (image variants, EXIF metadata).
  - R2 URL rewriter.
  Both are concrete-but-shape-uncertain: their contracts depend on which `sharp`-like image lib and which R2 client zzmod actually uses. Best written after a maintainer can copy from the zzmod source.
- **Verification-against-zzmod pass.** All three current seed recipes (`sitemap-from-ctx-routes`, `ogp-suffix-convention`, `virtual-metadata-db`) need a maintainer to re-run them against the current `zfb` build and flip `status: seed` to `status: verified`. Track as a follow-up issue once #358 is resolved.
- **Per-release re-verification automation.** A `scripts/recipes-check.ts` that walks `docs/src/content/docs/recipes/*.mdx`, parses frontmatter, and prints a table of `verified` dates older than `N` months. Cheap to write; high-leverage at release time.
- **i18n for recipes.** `docs/src/content/docs-ja/` exists for Japanese mirrors. For v0 the recipes are English-only; mirror to `docs-ja/recipes/` once the v0 set stabilises and is verified.
- **Cross-link from `concepts/plugins.mdx`.** Once the catalog is stable (i.e. at least one recipe is `status: verified`), add a "See also: Recipes" pointer at the bottom of `concepts/plugins.mdx` and `concepts/non-html-pages.mdx`. Issue #348 explicitly defers this until policy stability.
- **`semver` library opt-in.** If `applies_to_zfb_version` strings start drifting (e.g. typos like `">0.1.<0.2"`), add a `semver`-based range parser to `recipeSchema` and validate at build time. Skipped for v0 to avoid a new dependency.

## 6. Scope exceptions

Files touched outside the primary `research/348-recipes-catalog.md` deliverable. Each is within the file-scope guardrail in the task prompt, called out per the prompt's instruction:

- `docs/src/content.config.ts` — extended `docsSchema` with the optional strict `recipe` Zod sub-object. Required to give the catalog real schema enforcement (option (b) above).
- `docs/src/content/docs/recipes/_category_.json` — sidebar label + position. New directory.
- `docs/src/content/docs/recipes/index.mdx` — catalog overview / policy summary visible to docs readers.
- `docs/src/content/docs/recipes/sitemap-from-ctx-routes.mdx` — seed recipe 1.
- `docs/src/content/docs/recipes/ogp-suffix-convention.mdx` — seed recipe 2.
- `docs/src/content/docs/recipes/virtual-metadata-db.mdx` — seed recipe 3.

`docs/astro.config.ts` was **not** modified. The sidebar is driven by `_category_.json` files plus per-file `sidebar_position` — no `astro.config.ts` entry is required for a new top-level section.

The task prompt referred to `docs/astro.config.mjs`; the actual file in this repo is `docs/astro.config.ts`. Same role, different extension. No edit was needed either way.

## 8. Post-investigation revision (maintainer direction)

After the investigation completed, the maintainer revised the scope: the catalog should ship in a **WIP-only state** for now, with the actual recipe content authored later in a different format than the seed-recipe drafts produced here.

Reasoning (paraphrased):

- Recipe content needs to be **written as standalone explanation articles** — "1 topic, 1 article" — for someone who has not seen the source project the pattern came from.
- The zzmod-derived seed drafts were too close to "code snippets with frontmatter" and not enough like the explanation-shaped articles the maintainer wants. Re-authoring later is cheaper than retrofitting.
- The strict Zod schema is **deferred** for the same reason: shape it when the first real recipe is being written, not before.

### What was kept

- `docs/src/content/docs/recipes/` — the category directory
- `docs/src/content/docs/recipes/_category_.json` — sidebar label + position
- `docs/src/content/docs/recipes/index.mdx` — rewritten as a short placeholder that announces the WIP status, frames the eventual style (1 topic per article, explanation-shaped, maintainer-curated), and explicitly states the section is empty for now

### What was dropped

- The three seed recipes (`sitemap-from-ctx-routes.mdx`, `ogp-suffix-convention.mdx`, `virtual-metadata-db.mdx`) — removed in the same revision
- The strict `recipe` Zod object in `docs/src/content.config.ts` — reverted (the schema can return when real recipes are authored)

### What this means for the PR

The PR's deliverable narrows from "3 seed recipes + strict schema + policy doc" to "category slot + WIP index placeholder." The discipline analysis in §3 of this findings doc is preserved as a record of the trade-off space — it informs the future content but does not gate it.
