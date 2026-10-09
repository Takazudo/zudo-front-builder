import { defineConfig } from "vite-plus";

export default defineConfig({
  test: {
    clearMocks: false,
    environment: "node",
    include: ["src/**/__tests__/**/*.test.ts"],
  },
});
