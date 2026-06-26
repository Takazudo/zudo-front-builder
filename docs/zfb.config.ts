import { defineConfig } from "zfb/config";
import { zudoDocPreset } from "@takazudo/zudo-doc/preset";
import { settings } from "./src/config/settings";
import { buildDocsSchema } from "./src/config/docs-schema";

// The canonical seven directives for this showcase. Keys are directive names;
// values are the JSX component names registered in pages/_mdx-components.ts.
// "details" routes to DetailsWrapper — a collapsible, NOT an admonition.
const directiveVocabulary = {
  note: "Note",
  tip: "Tip",
  info: "Info",
  warning: "Warning",
  danger: "Danger",
  caution: "Caution",
  details: "Details",
};

export default defineConfig({
  framework: "preact",
  tailwind: { enabled: true },
  base: settings.base,
  adapter: "@takazudo/zfb-adapter-cloudflare",
  ...zudoDocPreset({ settings, buildDocsSchema, directiveVocabulary }),
});
