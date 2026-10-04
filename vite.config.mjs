import { defineConfig } from "vite-plus";

// Contributor tooling only (#3558): the `fmt` block configures Oxfmt for
// `vp fmt` (`pnpm format:ts` / `format:check:ts` and the lefthook `format-ts`
// hook). ZFB's product configuration is `zfb.config.ts`, never this file, and
// `vp test` keeps reading the `vitest.config.*` files.
//
// Ownership: Oxfmt owns js, mjs, cjs, ts, tsx, json, yml and yaml — the set
// Prettier formatted before. @takazudo/mdx-formatter alone owns .md and .mdx
// (`.mdx-formatter.json`), and Rust stays with `cargo fmt`.
export default defineConfig({
  fmt: {
    // The former .prettierrc.json settings.
    semi: true,
    singleQuote: false,
    trailingComma: "all",
    printWidth: 100,
    // Oxfmt sorts package.json keys by default; Prettier never did, and the
    // root scripts are grouped on purpose. Import sorting is off by default.
    sortPackageJson: false,
    ignorePatterns: [
      // Formats outside Oxfmt's ownership. Markdown/MDX belongs to
      // mdx-formatter; Prettier never formatted the rest (HTML/CSS goldens,
      // TOML, the JSX and YAML-syntax fixtures), so a bare `vp fmt` must not
      // either.
      "*.md",
      "*.mdx",
      "*.html",
      "*.css",
      "*.toml",
      "*.jsx",
      "*.sublime-syntax",
      // The former .prettierignore. Oxfmt also reads the root .gitignore, as
      // Prettier 3 did.
      "worktrees/",
      "node_modules/",
      "target/",
      "dist/",
      ".pnpm-store/",
      "pnpm-lock.yaml",
      "docs/dist/",
      ".playwright-cli/",
      "__inbox/",
      "docs/.zudo-doc/",
      // Generated wasm-bindgen glue (crates/zfb-md-wasm/npm/.gitignore), only
      // present on a tree that has built the wasm package.
      "crates/zfb-md-wasm/npm/src/wasm/",
      "crates/zfb-md-wasm/npm/src/wasm-highlight/",
      "crates/zfb-md-wasm/npm/src/wasm-render/",
      "crates/zfb-md-wasm/npm/src/wasm-parse/",
      // Snapshot of the .d.ts emitter's byte-exact output.
      "crates/zfb-content/tests/fixtures/two-collections-schemas/expected.d.ts",
      // Markdown sources and compiler output are byte-exact cross-language goldens.
      "packages/zfb/src/__tests__/zudo-react/fixtures/md-roundtrip/**",
      "_temp-resource/",
    ],
  },
});
