// Forward-looking sketch of the eventual `zfb.config.ts` form.
//
// The current zfb config loader (crates/zfb/src/config.rs) only accepts
// `zfb.config.json`; TS loading is gated on ADR-001 / the JS runtime
// decision. Until that ships, `zfb.config.json` is the source of truth
// and this file is **intentionally not validated** against the JSON
// sibling (e.g. it may omit `outDir` / `publicDir` and pick up the
// loader defaults). It exists today purely to type-check the eventual
// shape via `defineConfig`.

import { defineConfig } from "zfb/config";

export default defineConfig({
  framework: "preact",
  tailwind: {
    enabled: true,
  },
  collections: [
    {
      name: "blog",
      path: "content/blog",
    },
  ],
  // Deploy-target adapter. Omit (or set to "none") for a pure-static
  // build — the default for this example. Set to a package name like
  // "@takazudo/zfb-adapter-cloudflare" to produce dist/_worker.js
  // alongside the static HTML so SSR routes (`export const prerender =
  // false`) become deployable. Adding an SSR route without an adapter
  // is a hard build error.
  // adapter: "@takazudo/zfb-adapter-cloudflare",
});
