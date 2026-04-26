import { defineConfig } from "zfb/config";

export default defineConfig({
  framework: "preact",
  tailwind: {
    enabled: true,
  },
  collections: {
    blog: {
      type: "content",
      directory: "content/blog",
      schema: {
        title: "string",
        date: "date",
        description: "string?",
      },
    },
  },
});
