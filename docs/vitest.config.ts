import { defineConfig } from "vite-plus";

export default defineConfig({
  resolve: {
    alias: { "zfb/config": "@takazudo/zfb/config" },
  },
  test: {
    // Vitest v4 compatibility: preserve mock call history.
    // Remove after tests no longer rely on calls from setup or earlier tests.
    // https://viteplus.dev/guide/vitest-v5#remove-unneeded-compatibility-settings
    // https://vitest.dev/guide/migration/#clearmocks-is-enabled-by-default
    clearMocks: false,
    environment: "node",
    include: ["src/components/playground/__tests__/**/*.test.ts"],
  },
});
