// `zfb/config` — TypeScript helper for the `zfb.config.ts` form.
//
// The zfb config loader (`crates/zfb/src/config.rs`) accepts both
// `zfb.config.ts` and `zfb.config.json`; JSON wins when both files are
// present, which is the back-compat path for projects predating the TS
// loader. New projects should prefer the TS form for editor types and
// `defineConfig` autocomplete.
//
// At parse time, zfb bundles the user's `zfb.config.ts` with esbuild and
// aliases this `zfb/config` import to an internal stub that re-exports
// `defineConfig` as the identity function — so a user project does not
// need the `zfb` npm package installed locally just to be parsed.
//
// The shape mirrors the Rust `Config` struct one-for-one. Keep them in
// sync; the `defineConfig` identity helper is the single anchor point.

export type Framework = "preact" | "react";

export type CollectionDef = {
  /** Identifier used at the call site (e.g. `"blog"`). */
  name: string;
  /** Directory (relative to the project root) holding the entries. */
  path: string;
  /** Optional schema. Reserved for v1.1 — accepted but not enforced today. */
  schema?: Record<string, unknown>;
};

export type TailwindConfig = {
  /** Whether Tailwind is enabled. Default: `true`. */
  enabled?: boolean;
};

/**
 * One plugin entry in `zfb.config.ts`.
 *
 * `name` MUST be a module reference that Node's resolver can locate from
 * the project root. The zfb config loader (`crates/zfb/js/config-loader.mjs`)
 * resolves it to an absolute module specifier and the build / dev plugin
 * host loads it via dynamic `import()`:
 *
 * - `"./plugins/my-plugin.mjs"` / `"../shared/plugin.mjs"` —
 *   path-relative to the project root (the dir containing `zfb.config.ts`).
 * - `"/abs/path/to/plugin.mjs"` — absolute filesystem path.
 * - `"@takazudo/zfb-plugin-search"` / `"my-plugin"` — npm bare specifier
 *   resolved against the project's `node_modules`.
 *
 * Inline-function hooks are NOT supported; the plugin module's default
 * export must be a [`ZfbPlugin`] (see `@takazudo/zfb/plugins`).
 *
 * `options` is passed verbatim to the plugin's hook contexts; treat
 * the schema as plugin-specific.
 */
export type PluginConfig = {
  name: string;
  options?: Record<string, unknown>;
};

export type ZfbConfig = {
  /** Output directory for built assets. Default: `dist`. */
  outDir?: string;
  /** Public/static directory copied verbatim. Default: `public`. */
  publicDir?: string;
  /** Optional dev/preview server bind host. */
  host?: string;
  /** Optional dev/preview server port. */
  port?: number;
  /** JSX framework runtime. Default: `preact`. */
  framework?: Framework;
  /** Content collections. Mirrors the JSON form one-for-one. */
  collections?: CollectionDef[];
  /** Tailwind options; absent = defaults. */
  tailwind?: TailwindConfig;
  /** User-supplied plugins. */
  plugins?: PluginConfig[];
  /**
   * Deploy-target adapter package name. Omit (or `"none"`) for a pure
   * static build — any route exporting `prerender = false` is then a
   * hard build error. A package name like
   * `"@takazudo/zfb-adapter-cloudflare"` selects the matching adapter,
   * and `zfb build` invokes that package's bin to wrap the SSR bundle
   * into a deploy-ready entry (e.g. `dist/_worker.js` for Cloudflare
   * Pages).
   *
   * Mirrors `Config::adapter` in crates/zfb/src/config.rs.
   */
  adapter?: string;
  /**
   * Strip `.md` / `.mdx` from internal `<a href>` paths during MDX
   * compilation, and append a trailing `/` so the resulting URL shape
   * converges with the rest of the site (mirrors the JS engine's
   * `rehypeStripMdExtension`). Default: `false`.
   *
   * Enable this when content authors hand-write `[label](other.md)`
   * style references that should resolve to the rendered route URL
   * (e.g. `other/`) instead of a literal file path. Built dist and
   * `pnpm dev` honour the same flag, so previews match shipped output.
   *
   * Mirrors `Config::strip_md_ext` in crates/zfb/src/config.rs.
   */
  stripMdExt?: boolean;
};

/**
 * Identity helper: returns the supplied config as-is, but typed against
 * [`ZfbConfig`]. Use as the default export of `zfb.config.ts` so editors
 * surface field-level types and typos surface at compile time.
 */
export function defineConfig(config: ZfbConfig): ZfbConfig {
  return config;
}
