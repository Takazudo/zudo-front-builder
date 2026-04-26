// `zfb/config` — TypeScript helper for the `zfb.config.ts` form.
//
// The v0 loader (`crates/zfb/src/config.rs`) accepts `zfb.config.json` and
// hard-errors on `zfb.config.ts` until the JS-runtime decision (ADR-001)
// lands. This module exists so the future, typed `zfb.config.ts` form can
// be authored today against a real type — the basic-blog example pins one
// such file (`zfb.config.future.ts`) and uses it as the type-checked
// sibling to the JSON source of truth.
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
};

/**
 * Identity helper: returns the supplied config as-is, but typed against
 * [`ZfbConfig`]. Use as the default export of `zfb.config.ts` so editors
 * surface field-level types and typos surface at compile time.
 */
export function defineConfig(config: ZfbConfig): ZfbConfig {
  return config;
}
