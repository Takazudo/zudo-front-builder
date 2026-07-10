import { defineConfig } from "@takazudo/zfb/config";

export default defineConfig({
  framework: "preact",
  adapter: "@takazudo/zfb-adapter-cloudflare",
  outDir: "dist",
  publicDir: "public",
  tailwind: {
    enabled: true,
  },
});
