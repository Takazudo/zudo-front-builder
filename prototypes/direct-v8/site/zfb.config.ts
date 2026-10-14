import { defineConfig } from "zfb/config";

export default defineConfig({
  outDir: "dist",
  collections: [{ name: "posts", path: "content/posts" }],
});
