/**
 * Config type fixture.
 *
 * Compiled by `tsc -p tsconfig.preact-fixture.json` through the package's
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

export const bundleRejectsAssetEmittingLoaders = defineConfig({
  bundle: {
    loaders: {
      // @ts-expect-error `file` emits a sibling asset and is not supported.
      ".png": "file",
    },
  },
});
