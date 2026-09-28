import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

export default defineConfig({
  resolve: {
    alias: {
      "@takazudo/zfb/jsx-factory": fileURLToPath(
        new URL("../zfb/src/zudo-react/jsx-runtime.ts", import.meta.url),
      ),
    },
  },
  test: {
    environment: "node",
    include: ["src/**/__tests__/**/*.test.ts"],
  },
});
