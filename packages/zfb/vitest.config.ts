import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

export default defineConfig({
  resolve: {
    alias: {
      "@takazudo/zfb/jsx-factory": fileURLToPath(
        new URL("./src/zudo-react/jsx-runtime.ts", import.meta.url),
      ),
      "@takazudo/zfb/island-boundary": fileURLToPath(
        new URL("./src/island-boundary-zudo-react.ts", import.meta.url),
      ),
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/__tests__/**/*.test.ts"],
  },
});
