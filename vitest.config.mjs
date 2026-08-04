import { defineConfig } from "vitest/config";

// Root-level config for tests over root-level scripts/** (e.g.
// scripts/showcase-inject-banner.mjs, issue #2282). Every other package in
// this workspace (packages/*) carries its own scoped vitest.config — this
// one is scoped the same way, to scripts/**/__tests__, so it never picks up
// those packages' test files (which may need a different environment, e.g.
// happy-dom for zfb-runtime).
export default defineConfig({
  test: {
    environment: "node",
    include: ["scripts/**/__tests__/**/*.test.mjs"],
  },
});
