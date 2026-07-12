import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts"],
    // The reinit test forces a real wasm trap and re-instantiates the
    // module; give it more headroom than the default 5s.
    testTimeout: 15000,
  },
});
