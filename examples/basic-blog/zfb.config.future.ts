import { defineConfig } from "zfb/config";

// NOTE (v0): this file is intentionally named `zfb.config.future.ts` rather
// than `zfb.config.ts`. The current zfb config loader hard-errors if it
// finds a `zfb.config.ts` next to a `zfb.config.json` (see crates/zfb/src/
// config.rs — TS loading is gated on ADR-001 / the JS runtime). Once the TS
// loader lands, rename this to `zfb.config.ts` and delete the JSON sibling.
// Until then, `zfb.config.json` is the source of truth and this file is the
// canonical, type-checked shape kept in sync by hand for review purposes.
export default defineConfig({
  framework: "preact",
  tailwind: {
    enabled: true,
  },
  collections: {
    blog: {
      type: "content",
      directory: "content/blog",
      schema: {
        title: "string",
        date: "date",
        description: "string?",
        tags: "string[]?",
      },
    },
  },
});
