// Canonical TS form of the zfb config — the recommended way to author
// a zfb project's configuration.
//
// The zfb config loader (crates/zfb/src/config.rs) accepts both
// `zfb.config.ts` and `zfb.config.json`. TS wins when both files are
// present — the TypeScript form is the canonical path for new projects.
//
// TS configs are bundled by the staged esbuild binary
// (crates/zfb/binaries/esbuild/esbuild, also overridable via
// `ZFB_ESBUILD_BIN`) and then evaluated by `node` to pull the default
// export back as JSON — `node` must therefore be in `PATH` (already a
// hard requirement of zfb because the production renderer spawns
// miniflare). The user's `import { defineConfig } from "@takazudo/zfb/config"`
// is satisfied by an internal stub so the project does NOT need the
// `zfb` npm package installed locally just to be parsed.

import { defineConfig } from "@takazudo/zfb/config";

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
