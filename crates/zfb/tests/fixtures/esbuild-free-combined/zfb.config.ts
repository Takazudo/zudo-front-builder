import { defineConfig } from "zfb/config";

// #4636: the TypeScript config itself is bundled by the embedded Rolldown
// backend. The type-only constructs below must be erased, and the computed
// define value must reach the rendered page.
type ConfigMode = "combined";
const mode: ConfigMode = "combined";
const configMarker = `ZFB_TS_CONFIG:${mode satisfies ConfigMode}`;

export default defineConfig({
  wind: false,
  define: {
    __COMBINED_CONFIG_MARKER__: JSON.stringify(configMarker),
  },
  plugins: [{ name: "./plugins/stamp-plugin.ts" }],
});
