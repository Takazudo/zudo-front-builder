import { defineConfig } from "vite-plus";

export default defineConfig({
  test: {
    // Vitest v4 compatibility: preserve mock call history.
    // Remove after tests no longer rely on calls from setup or earlier tests.
    // https://viteplus.dev/guide/vitest-v5#remove-unneeded-compatibility-settings
    // https://vitest.dev/guide/migration/#clearmocks-is-enabled-by-default
    clearMocks: false,
    // 2026-09 concurrent-workspace re-measure (#3067): K=1..4, 42 batches, 1944 CLI JSON durations + 780 sparse workspace durations; max cli.test.ts=9259ms at K=4 (JSON max=6302.24ms; 185.18% of old 5s, 48.73% of 19s); testTimeout=19000ms.
    testTimeout: 19_000,
    environment: "node",
    include: ["src/**/__tests__/**/*.test.ts"],
  },
});
