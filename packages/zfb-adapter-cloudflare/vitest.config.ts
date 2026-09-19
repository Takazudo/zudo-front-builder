import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // 2026-09 concurrent-workspace re-measure (#3067): K=1..4, 42 batches, 1944 CLI JSON durations + 780 sparse workspace durations; max cli.test.ts=9259ms at K=4 (JSON max=6302.24ms; 185.18% of old 5s, 48.73% of 19s); testTimeout=19000ms.
    testTimeout: 19_000,
    environment: "node",
    include: ["src/**/__tests__/**/*.test.ts"],
  },
});
