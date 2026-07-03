import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["bin/**/__tests__/**/*.test.mjs"],
  },
});
