import { defineConfig } from "@takazudo/zfb/config";

export default defineConfig({
  framework: "preact",
  adapter: "@takazudo/zfb-adapter-cloudflare",
  tailwind: {
    enabled: true,
  },
});
