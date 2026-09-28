// AUTO-LOADED by zfb::config during `zfb.config.ts` evaluation.
// Used on both the default (embed_v8) in-process path and the slim-build
// subprocess fallback (`load_ts_via_subprocess`). Do not edit unless you
// also update the Rust callers in `crates/zfb/src/config.rs`.
//
// Stub for the `zfb/config` import that user `zfb.config.ts` files
// reach for. The real package (packages/zfb/src/config.ts) exposes
// `defineConfig` and `definePreset` as identity/stamping helpers plus
// pure type aliases. At config-load time we only care about the runtime
// value, so this stub re-implements those helpers and ignores the type
// surface.
//
// We inject this stub via esbuild's `--alias:zfb/config=<this-file>`
// so the user's `zfb.config.ts` does NOT need the `zfb` npm package
// installed locally just to be parsed.
//
// SYNC REQUIREMENT: keep definePreset here behaviourally identical to
// packages/zfb/src/config.ts. The `source_package` plugin field matches the
// Rust `PluginConfig`; `__zfb_source_package` on wind manifests is stripped by
// the Rust config loader before strict public-schema deserialization.

export function defineConfig(config) {
  return config;
}

export function definePreset(sourcePackage, config) {
  let sourceStampedConfig = config;
  const wind = config.wind;
  if (wind && wind !== false && wind.manifests) {
    const manifests = Object.fromEntries(
      Object.entries(wind.manifests).map(([producer, manifest]) => {
        if (manifest !== null && typeof manifest === "object" && !Array.isArray(manifest)) {
          // Default first so a composed inner preset's source marker wins.
          return [producer, { __zfb_source_package: sourcePackage, ...manifest }];
        }
        return [producer, manifest];
      }),
    );
    sourceStampedConfig = {
      ...config,
      wind: { ...wind, manifests },
    };
  }

  if (!sourceStampedConfig.plugins) {
    return sourceStampedConfig;
  }
  return {
    ...sourceStampedConfig,
    plugins: sourceStampedConfig.plugins.map((plugin) => {
      if (plugin !== null && typeof plugin === "object" && !Array.isArray(plugin)) {
        // Default first, then spread so an existing `source_package` (from a
        // composed inner preset) wins over the outer package name.
        return { source_package: sourcePackage, ...plugin };
      }
      return plugin;
    }),
  };
}
