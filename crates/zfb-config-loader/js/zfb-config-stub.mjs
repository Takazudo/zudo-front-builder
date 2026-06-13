// AUTO-LOADED by zfb::config during `zfb.config.ts` evaluation.
// Used on both the default (embed_v8) in-process path and the slim-build
// subprocess fallback (`load_ts_via_subprocess`). Do not edit unless you
// also update the Rust callers in `crates/zfb/src/config.rs`.
//
// Stub for the `zfb/config` import that user `zfb.config.ts` files
// reach for. The real package (packages/zfb/src/config.ts) exposes
// `defineConfig` as an identity helper plus pure type aliases. At
// config-load time we only care about the runtime value, so this stub
// re-implements `defineConfig` as the identity function and ignores
// the type surface.
//
// We inject this stub via esbuild's `--alias:zfb/config=<this-file>`
// so the user's `zfb.config.ts` does NOT need the `zfb` npm package
// installed locally just to be parsed.

export function defineConfig(config) {
  return config;
}
