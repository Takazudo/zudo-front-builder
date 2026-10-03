/**
 * Config type fixture.
 *
 * Compiled by `tsc -p tsconfig.config-fixture.json` through the package's
 * type-test include. Keep this file compile-only; it proves config
 * helper types reject invalid shapes without adding runtime test code.
 */

import { defineConfig } from "../src/config.js";

export const bundleInlineLoadersAndRawDefines = defineConfig({
  bundle: {
    loaders: {
      ".txt": "text",
      ".bin": "binary",
    },
    define: {
      __APP_NAME__: '"zfb"',
      __FEATURE_ENABLED__: "true",
    },
  },
});

// @ts-expect-error the framework selector was removed; zfb uses zudo-react.
export const removedFramework = defineConfig({ framework: "zudo-react" });

export const bundleRejectsAssetEmittingLoaders = defineConfig({
  bundle: {
    loaders: {
      // @ts-expect-error `file` emits a sibling asset and is not supported.
      ".png": "file",
    },
  },
});

export const windAcceptsFullConfiguration = defineConfig({
  wind: {
    spec: 1,
    reset: "none",
    defaultTransitionTimingFunction: "cubic-bezier(0.2, 0, 0, 1)",
    tokens: {
      spacingUnit: "0.25rem",
      colors: { panel: "var(--project-panel)" },
      fontSizes: { small: { size: "0.875rem", lineHeight: "1.25rem" } },
      easings: { gentle: "ease-in-out" },
    },
    breakpoints: { sm: { minWidthPx: 640 } },
    dark: { attribute: "data-theme", value: "dark" },
    safelist: { app: ["sm:hover:bg-panel", "rounded"] },
    authoredClasses: { prose: true },
    manifests: { widgets: { path: "@example/widgets/wind.json" } },
    sources: { exclude: ["src/**/__tests__/**"], packageRoots: ["@example/widgets"] },
  },
});

export const windCanBeDisabled = defineConfig({ wind: false });

export const windRejectsUnsupportedSpec = defineConfig({
  // @ts-expect-error zudo-wind currently supports only configuration version 1.
  wind: { spec: 2 },
});

export const windRejectsTrueShorthand = defineConfig({
  // @ts-expect-error wind is either false or a configuration object.
  wind: true,
});

export const tailwindKeyIsRejected = defineConfig({
  // @ts-expect-error Tailwind configuration was removed in zfb 3.
  tailwind: { enabled: false },
});
