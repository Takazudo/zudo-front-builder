import { defineConfig } from "zfb/config";
import { zudoDocPreset } from "@takazudo/zudo-doc/preset";
import { settings } from "./src/config/settings";
import { buildDocsSchema } from "./src/config/docs-schema";
import { translations } from "./src/config/i18n";
import { colorSchemes } from "./src/config/color-schemes";

// The canonical seven directives for this showcase. Keys are directive names;
// values are the JSX component names — under zudo-doc package-owned routes,
// these all resolve to package-provided mdx-components (deriveMdxComponents),
// so the host no longer registers them. "details" is a collapsible, NOT an
// admonition.
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
  // translations + colorSchemes are consumed by zudo-doc package-owned
  // routes (packageOwnedRoutes) so /404 etc. inherit host i18n/theme; optional
  // in v1, required-in-practice once packageOwnedRoutes is on (zudo-doc #2404).
  ...zudoDocPreset({ settings, buildDocsSchema, directiveVocabulary, translations, colorSchemes }),
});
