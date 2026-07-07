/**
 * Config type fixture.
 *
 * Compiled by `tsc -p tsconfig.preact-fixture.json` through the package's
 * type-test include. Keep this file compile-only; it proves config
 * helper types reject invalid shapes without adding runtime test code.
 */

import { defineConfig } from "../src/config.js";

export const githubAutolinksWithRepo = defineConfig({
  markdown: {
    features: {
      githubAutolinks: {
        repo: "owner/repo",
      },
    },
  },
});

export const githubAutolinksOmitted = defineConfig({
  markdown: {
    features: {},
  },
});

export const githubAutolinksEmptyObject = defineConfig({
  markdown: {
    features: {
      // @ts-expect-error repo is required when githubAutolinks is configured.
      githubAutolinks: {},
    },
  },
});
