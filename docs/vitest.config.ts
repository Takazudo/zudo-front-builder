import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: { "zfb/config": "@takazudo/zfb/config" },
  },
  test: {
    environment: "node",
    include: ["src/components/playground/__tests__/**/*.test.ts"],
  },
});
