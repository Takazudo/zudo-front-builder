/**
 * `zfb.config.ts` imports `defineConfig` from the bare specifier
 * `zfb/config`, which zfb's config loader aliases to an internal stub at
 * parse time — nothing resolves it on disk, so TypeScript would report the
 * import as unresolved. This maps it onto the real types shipped by the
 * installed package so editors and `zfb check` see the full config shape.
 */
declare module "zfb/config" {
  export * from "@takazudo/zfb/config";
}
